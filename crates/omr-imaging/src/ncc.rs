//! Normalised cross-correlation (NCC) with exact integer accumulation (§8.11: identical results on
//! every ISA), a zero-variance guard (flat windows score 0, never 1), local maxima with parabolic
//! sub-pixel refinement, and Gaussian ring templates for bubble detection (§8.4 Stage D).
//!
//! All NCC values here are the coefficient of the **zero-mean template** with the window
//! (OpenCV `TM_CCOEFF_NORMED`): `Σ (t − t̄)(x − x̄) / (‖t − t̄‖ · ‖x − x̄‖)`. Sums are exact
//! integers; only the final division and square roots are floating point.

use crate::gray::GrayImage;
use omr_model::PointPx;

/// The default flat-window guard: windows whose variance (on a 0–1 intensity scale) is at most
/// `2·10⁻³` score 0 (the prototype's guard; without it flat paper scores NCC = 1).
pub const FLAT_VARIANCE: f64 = 2e-3;

#[inline]
fn is_flat(n: f64, nvar: f64, flat_var: f64) -> bool {
    // nvar = n·Σx² − (Σx)² = n²·var (255² units)
    nvar <= flat_var * 65_025.0 * n * n
}

/// Summed-area tables of `v` and `v²`.
#[derive(Clone, Debug)]
pub struct Integral {
    w1: usize,
    s: Vec<u64>,
    s2: Vec<u64>,
}

impl Integral {
    /// Builds the tables for `img`.
    pub fn new(img: &GrayImage) -> Self {
        let (w, h) = (img.width(), img.height());
        let w1 = w + 1;
        let mut s = vec![0u64; w1 * (h + 1)];
        let mut s2 = vec![0u64; w1 * (h + 1)];
        for y in 0..h {
            let (mut a, mut b) = (0u64, 0u64);
            let row = img.row(y);
            for x in 0..w {
                let v = u64::from(row[x]);
                a += v;
                b += v * v;
                s[(y + 1) * w1 + x + 1] = s[y * w1 + x + 1] + a;
                s2[(y + 1) * w1 + x + 1] = s2[y * w1 + x + 1] + b;
            }
        }
        Self { w1, s, s2 }
    }

    /// `(Σv, Σv²)` over the `w × h` window at top-left `(x, y)` (must be inside).
    #[inline]
    pub fn sums(&self, x: usize, y: usize, w: usize, h: usize) -> (u64, u64) {
        let a = y * self.w1 + x;
        let b = y * self.w1 + x + w;
        let c = (y + h) * self.w1 + x;
        let d = (y + h) * self.w1 + x + w;
        (
            self.s[d] + self.s[a] - self.s[b] - self.s[c],
            self.s2[d] + self.s2[a] - self.s2[b] - self.s2[c],
        )
    }
}

/// An image patch used as an NCC template.
#[derive(Clone, Debug)]
pub struct PatchTemplate {
    w: usize,
    h: usize,
    data: Vec<u8>,
    sum: u64,
    nvar: f64,
}

impl PatchTemplate {
    /// Uses the whole of `img` as the template.
    pub fn new(img: &GrayImage) -> Self {
        let data = img.as_raw().to_vec();
        let n = data.len() as u64;
        let sum: u64 = data.iter().map(|&v| u64::from(v)).sum();
        let sum2: u64 = data.iter().map(|&v| u64::from(v) * u64::from(v)).sum();
        Self {
            w: img.width(),
            h: img.height(),
            data,
            sum,
            nvar: (n as f64) * (sum2 as f64) - (sum as f64) * (sum as f64),
        }
    }

    /// Template width.
    pub fn width(&self) -> usize {
        self.w
    }

    /// Template height.
    pub fn height(&self) -> usize {
        self.h
    }
}

#[inline]
fn row_dot(a: &[u8], b: &[u8]) -> u64 {
    // ≤ 65,025 per term; chunks of 4,096 keep the u32 partial sums exact.
    let mut total = 0u64;
    for (ca, cb) in a.chunks(4096).zip(b.chunks(4096)) {
        let s: u32 = ca
            .iter()
            .zip(cb)
            .map(|(&x, &y)| u32::from(x) * u32::from(y))
            .sum();
        total += u64::from(s);
    }
    total
}

/// NCC of `tpl` placed with its top-left at `(x0, y0)` in `img` (`None` if it does not fit).
pub fn ncc_patch(
    img: &GrayImage,
    integral: &Integral,
    tpl: &PatchTemplate,
    x0: isize,
    y0: isize,
    flat_var: f64,
) -> Option<f64> {
    if x0 < 0 || y0 < 0 || x0 as usize + tpl.w > img.width() || y0 as usize + tpl.h > img.height() {
        return None;
    }
    let (x0, y0) = (x0 as usize, y0 as usize);
    let mut dot = 0u64;
    for r in 0..tpl.h {
        let irow = &img.row(y0 + r)[x0..x0 + tpl.w];
        let trow = &tpl.data[r * tpl.w..(r + 1) * tpl.w];
        dot += row_dot(irow, trow);
    }
    let (sx, sx2) = integral.sums(x0, y0, tpl.w, tpl.h);
    let n = (tpl.w * tpl.h) as f64;
    let nvar_x = n * sx2 as f64 - (sx as f64) * (sx as f64);
    if is_flat(n, nvar_x, flat_var) || tpl.nvar <= 0.0 {
        return Some(0.0);
    }
    let num = n * dot as f64 - (tpl.sum as f64) * (sx as f64);
    Some(num / (tpl.nvar.sqrt() * nvar_x.sqrt()))
}

/// A grid of NCC scores over template top-left positions.
#[derive(Clone, Debug)]
pub struct NccGrid {
    /// x of the first column (top-left position of the template).
    pub x0: isize,
    /// y of the first row.
    pub y0: isize,
    /// Columns.
    pub w: usize,
    /// Rows.
    pub h: usize,
    /// Scores, row-major; `NEG_INFINITY` where the template does not fit.
    pub data: Vec<f64>,
}

impl NccGrid {
    /// The score at absolute position `(x, y)`, if inside the grid and valid.
    pub fn get(&self, x: isize, y: isize) -> Option<f64> {
        if x < self.x0 || y < self.y0 {
            return None;
        }
        let (i, j) = ((x - self.x0) as usize, (y - self.y0) as usize);
        if i >= self.w || j >= self.h {
            return None;
        }
        let v = self.data[j * self.w + i];
        v.is_finite().then_some(v)
    }

    /// The best position `(x, y, score)`; ties go to the first in row-major order.
    pub fn argmax(&self) -> Option<(isize, isize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, &v) in self.data.iter().enumerate() {
            if v.is_finite() && best.is_none_or(|(_, b)| v > b) {
                best = Some((i, v));
            }
        }
        best.map(|(i, v)| {
            (
                self.x0 + (i % self.w) as isize,
                self.y0 + (i / self.w) as isize,
                v,
            )
        })
    }
}

/// Exhaustive NCC of `tpl` over top-left positions `xs × ys` in `img`.
pub fn match_patch(
    img: &GrayImage,
    tpl: &PatchTemplate,
    xs: std::ops::Range<isize>,
    ys: std::ops::Range<isize>,
    flat_var: f64,
) -> NccGrid {
    let integral = Integral::new(img);
    let w = (xs.end - xs.start).max(0) as usize;
    let h = (ys.end - ys.start).max(0) as usize;
    let mut data = vec![f64::NEG_INFINITY; w * h];
    for (j, y) in ys.clone().enumerate() {
        for (i, x) in xs.clone().enumerate() {
            if let Some(v) = ncc_patch(img, &integral, tpl, x, y, flat_var) {
                data[j * w + i] = v;
            }
        }
    }
    NccGrid {
        x0: xs.start,
        y0: ys.start,
        w,
        h,
        data,
    }
}

/// The vertex offset of the parabola through `(−1, l)`, `(0, c)`, `(1, r)` (0 if degenerate).
#[inline]
pub fn parabolic_offset(l: f64, c: f64, r: f64) -> f64 {
    let den = l - 2.0 * c + r;
    if den.abs() > 1e-9 {
        0.5 * (l - r) / den
    } else {
        0.0
    }
}

/// A dense Gaussian ring template (the prototype's `ring_template`): side `⌊2R + 8⌋`, centre at
/// side/2, weight `exp(−((d − R)/w)²)` at each pixel centre, quantised to 1/1024 and kept where
/// ≥ 1/128 of the peak (the dropped tail changes NCC by < 1 %).
#[derive(Clone, Debug)]
pub struct RingKernel {
    radius: f64,
    width: f64,
    size: usize,
    taps: Vec<(u16, u16, i32)>,
    sum_w: i64,
    nvar_t: f64,
}

/// A refined ring detection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RingHit {
    /// Sub-pixel ring centre.
    pub centre: PointPx,
    /// NCC of the dark ring at the integer peak.
    pub score: f64,
}

impl RingKernel {
    /// The Gaussian ring of `radius` and `width` (px).
    pub fn gaussian(radius: f64, width: f64) -> Self {
        let size = (2.0 * radius + 8.0).floor().max(3.0) as usize;
        let c = size as f64 / 2.0;
        let mut taps = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f64 + 0.5 - c).powi(2) + (y as f64 + 0.5 - c).powi(2)).sqrt();
                let g = (-((d - radius) / width).powi(2)).exp();
                let q = (g * 1024.0).round() as i32;
                if q >= 8 {
                    taps.push((x as u16, y as u16, q));
                }
            }
        }
        let n = (size * size) as f64;
        let sum_w: i64 = taps.iter().map(|t| i64::from(t.2)).sum();
        let sum_w2: i64 = taps.iter().map(|t| i64::from(t.2) * i64::from(t.2)).sum();
        Self {
            radius,
            width,
            size,
            taps,
            sum_w,
            nvar_t: n * sum_w2 as f64 - (sum_w as f64) * (sum_w as f64),
        }
    }

    /// Ring radius.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Ring width parameter.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Template side in pixels.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Offset from the template's top-left to the ring centre (`size / 2`).
    pub fn centre_offset(&self) -> f64 {
        self.size as f64 / 2.0
    }

    /// NCC of a **dark** ring (i.e. of the inverted image with the ring) for the template
    /// top-left positions `[x0, x0 + nx) × [y0, y0 + ny)`; `None` per position that does not fit.
    pub fn ncc_dark_grid(
        &self,
        img: &GrayImage,
        x0: isize,
        y0: isize,
        nx: usize,
        ny: usize,
        flat_var: f64,
    ) -> Vec<Option<f64>> {
        let s = self.size;
        let pw = nx + s - 1;
        let ph = ny + s - 1;
        let patch = img.crop(x0, y0, pw, ph, 255);
        let integral = Integral::new(&patch);
        let n = (s * s) as f64;
        let mut out = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let (gx, gy) = (x0 + i as isize, y0 + j as isize);
                if gx < 0
                    || gy < 0
                    || gx as usize + s > img.width()
                    || gy as usize + s > img.height()
                {
                    out.push(None);
                    continue;
                }
                let (sx, sx2) = integral.sums(i, j, s, s);
                let nvar_x = n * sx2 as f64 - (sx as f64) * (sx as f64);
                if is_flat(n, nvar_x, flat_var) {
                    out.push(Some(0.0));
                    continue;
                }
                let d = patch.as_raw();
                let dot: i64 = if self.taps.len() <= 8_000 {
                    // ≤ 1,024 · 255 per tap: i32 is exact for up to 8,000 taps.
                    let mut acc = 0i32;
                    for &(tx, ty, w) in &self.taps {
                        acc += w * i32::from(d[(j + ty as usize) * pw + i + tx as usize]);
                    }
                    i64::from(acc)
                } else {
                    let mut acc = 0i64;
                    for &(tx, ty, w) in &self.taps {
                        acc +=
                            i64::from(w) * i64::from(d[(j + ty as usize) * pw + i + tx as usize]);
                    }
                    acc
                };
                let num = n * dot as f64 - (self.sum_w as f64) * (sx as f64);
                out.push(Some(-num / (self.nvar_t.sqrt() * nvar_x.sqrt())));
            }
        }
        out
    }

    /// Dark-ring NCC at one top-left position.
    pub fn ncc_dark_at(&self, img: &GrayImage, x0: isize, y0: isize, flat_var: f64) -> Option<f64> {
        self.ncc_dark_grid(img, x0, y0, 1, 1, flat_var)[0]
    }

    /// Climbs to the local NCC maximum near `guess` (a ring centre) on a 3 × 3 window that
    /// re-centres on its best cell up to `max_moves` times, then refines the peak with
    /// independent parabolic fits in x and y. `None` if the window leaves the image.
    pub fn refine(
        &self,
        img: &GrayImage,
        guess: PointPx,
        flat_var: f64,
        max_moves: usize,
    ) -> Option<RingHit> {
        let c = self.centre_offset();
        let mut x = (guess.x - c).round() as isize;
        let mut y = (guess.y - c).round() as isize;
        let mut v = [0.0f64; 9];
        for step in 0..=max_moves {
            let g = self.ncc_dark_grid(img, x - 1, y - 1, 3, 3, flat_var);
            for (o, s) in v.iter_mut().zip(&g) {
                *o = (*s)?;
            }
            let mut bi = 4;
            for (i, &s) in v.iter().enumerate() {
                if s > v[bi] {
                    bi = i;
                }
            }
            if bi != 4 && step < max_moves {
                x += (bi % 3) as isize - 1;
                y += (bi / 3) as isize - 1;
                continue;
            }
            let (bx, by) = (bi % 3, bi / 3);
            let cv = v[bi];
            let ox = if bx == 1 {
                parabolic_offset(v[3], cv, v[5])
            } else {
                0.0
            };
            let oy = if by == 1 {
                parabolic_offset(v[1], cv, v[7])
            } else {
                0.0
            };
            return Some(RingHit {
                centre: PointPx {
                    x: (x + bx as isize - 1) as f64 + ox + c,
                    y: (y + by as isize - 1) as f64 + oy + c,
                },
                score: cv,
            });
        }
        None
    }
}

/// A dense response map indexed by pixel; value `(i, j)` belongs to the point
/// `(i + origin, j + origin)` in image coordinates.
#[derive(Clone, Debug)]
pub struct ResponseMap {
    /// Width.
    pub width: usize,
    /// Height.
    pub height: usize,
    /// Point offset of index `(0, 0)`.
    pub origin: f64,
    /// Row-major responses (0 where undefined).
    pub data: Vec<f32>,
}

impl ResponseMap {
    /// Value at `(x, y)`.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }
}

/// A local maximum of a [`ResponseMap`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    /// Integer column.
    pub x: usize,
    /// Integer row.
    pub y: usize,
    /// Response at the integer peak.
    pub value: f32,
    /// Sub-pixel point (parabolic refinement, image coordinates).
    pub point: PointPx,
}

/// Points whose response exceeds `threshold` and equals the maximum of the `window × window`
/// neighbourhood centred on them (`window` odd), refined by parabolic fits; ordered by (y, x).
pub fn local_maxima(map: &ResponseMap, window: usize, threshold: f32) -> Vec<Peak> {
    let (w, h) = (map.width, map.height);
    let half = window / 2;
    let mut out = Vec::new();
    if w < 3 || h < 3 {
        return out;
    }
    for y in 1..h - 1 {
        let row = &map.data[y * w..(y + 1) * w];
        for (ci, chunk) in row.chunks(16).enumerate() {
            // Vectorisable pre-test: skip chunks with nothing above the threshold.
            if !chunk.iter().fold(false, |a, &v| a | (v > threshold)) {
                continue;
            }
            for (k, &v) in chunk.iter().enumerate() {
                let x = ci * 16 + k;
                if x == 0 || x + 1 >= w || v <= threshold {
                    continue;
                }
                // 3 × 3 pre-check, then the full window.
                let mut ok = true;
                'n: for yy in y - 1..=y + 1 {
                    for xx in x - 1..=x + 1 {
                        if map.get(xx, yy) > v {
                            ok = false;
                            break 'n;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let (ya, yb) = (y.saturating_sub(half), (y + half).min(h - 1));
                let (xa, xb) = (x.saturating_sub(half), (x + half).min(w - 1));
                'w: for yy in ya..=yb {
                    for &u in &map.data[yy * w + xa..=yy * w + xb] {
                        if u > v {
                            ok = false;
                            break 'w;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let c = f64::from(v);
                let ox = parabolic_offset(f64::from(row[x - 1]), c, f64::from(row[x + 1]));
                let oy = parabolic_offset(
                    f64::from(map.get(x, y - 1)),
                    c,
                    f64::from(map.get(x, y + 1)),
                );
                out.push(Peak {
                    x,
                    y,
                    value: v,
                    point: PointPx {
                        x: x as f64 + ox + map.origin,
                        y: y as f64 + oy + map.origin,
                    },
                });
            }
        }
    }
    out
}

/// Integer offsets on a circle of `radius`, deduplicated and ordered by (dy, dx).
pub fn circle_offsets(radius: f64) -> Vec<(i32, i32)> {
    let k = ((4.0 * std::f64::consts::PI * radius).ceil() as usize).max(24);
    let mut v: Vec<(i32, i32)> = (0..k)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / k as f64;
            let (s, c) = a.sin_cos();
            ((radius * c).round() as i32, (radius * s).round() as i32)
        })
        .collect();
    v.sort_by_key(|&(dx, dy)| (dy, dx));
    v.dedup();
    v
}

/// 3 × 3 binomial blur of the *inverted* image (`255 − v`), edges replicated, rounded:
/// `(Σ k · (255 − v) + 8) >> 4` with `k = [1 2 1] ⊗ [1 2 1]`.
fn inverted_binomial3(img: &GrayImage) -> Vec<u8> {
    let (w, h) = (img.width(), img.height());
    let mut tmp = vec![0u16; w * h];
    for y in 0..h {
        let r = img.row(y);
        let t = &mut tmp[y * w..(y + 1) * w];
        if w == 1 {
            t[0] = 4 * u16::from(r[0]);
            continue;
        }
        t[0] = 3 * u16::from(r[0]) + u16::from(r[1]);
        t[w - 1] = 3 * u16::from(r[w - 1]) + u16::from(r[w - 2]);
        for (((o, &a), &b), &c) in t[1..w - 1]
            .iter_mut()
            .zip(&r[..w - 2])
            .zip(&r[1..w - 1])
            .zip(&r[2..])
        {
            *o = u16::from(a) + 2 * u16::from(b) + u16::from(c);
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let up = &tmp[y.saturating_sub(1) * w..][..w];
        let mid = &tmp[y * w..][..w];
        let dn = &tmp[(y + 1).min(h - 1) * w..][..w];
        for (((o, &a), &b), &c) in out[y * w..(y + 1) * w].iter_mut().zip(up).zip(mid).zip(dn) {
            let s = u32::from(a) + 2 * u32::from(b) + u32::from(c);
            *o = ((4088 - s) >> 4) as u8;
        }
    }
    out
}

/// A fast dense map of dark-ring NCC for candidate search: the inverted image is blurred with a
/// 3 × 3 binomial kernel and summed over integer offsets on a circle of `radius`, which equals
/// correlation with a soft ring (σ ≈ 0.75 px); window sums give the exact NCC normalisation.
/// Indexed by centre pixel (`origin = 0.5`); border pixels whose window leaves the image are 0.
pub fn ring_map_fast(img: &GrayImage, radius: f64, flat_var: f64) -> ResponseMap {
    let (w, h) = (img.width(), img.height());
    let mut map = ResponseMap {
        width: w,
        height: h,
        origin: 0.5,
        data: vec![0.0; w * h],
    };
    let offs = circle_offsets(radius);
    let reach = offs
        .iter()
        .map(|&(dx, dy)| dx.unsigned_abs().max(dy.unsigned_abs()))
        .max()
        .unwrap_or(0) as usize;
    let half = reach + 1;
    let s = 2 * half + 1;
    if w < s || h < s || offs.len() * 255 > u16::MAX as usize || s * s * 65_025 > u32::MAX as usize
    {
        return map;
    }
    let n = (s * s) as f64;
    // Effective template: Σ_o b(· − o) on the s × s window.
    let b = [1.0, 2.0, 1.0];
    let mut g = vec![0.0f64; s * s];
    for &(dx, dy) in &offs {
        for (ky, by) in b.iter().enumerate() {
            for (kx, bx) in b.iter().enumerate() {
                let x = (half as i32 + dx + kx as i32 - 1) as usize;
                let y = (half as i32 + dy + ky as i32 - 1) as usize;
                g[y * s + x] += by * bx / 16.0;
            }
        }
    }
    let sum_g: f64 = g.iter().sum();
    let sum_g2: f64 = g.iter().map(|v| v * v).sum();
    let tnorm = (sum_g2 - sum_g * sum_g / n).sqrt() as f32;
    let gbar = (sum_g / n) as f32;
    let blur = inverted_binomial3(img);
    // Rolling column sums of the inverted image over the window rows.
    let mut col = vec![0u32; w];
    let mut col2 = vec![0u32; w];
    let add_row = |col: &mut [u32], col2: &mut [u32], y: usize| {
        for ((c, c2), &v) in col.iter_mut().zip(col2.iter_mut()).zip(img.row(y)) {
            let v = 255 - u32::from(v);
            *c += v;
            *c2 += v * v;
        }
    };
    let sub_row = |col: &mut [u32], col2: &mut [u32], y: usize| {
        for ((c, c2), &v) in col.iter_mut().zip(col2.iter_mut()).zip(img.row(y)) {
            let v = 255 - u32::from(v);
            *c -= v;
            *c2 -= v * v;
        }
    };
    for y in 0..s - 1 {
        add_row(&mut col, &mut col2, y);
    }
    let span = w - 2 * half;
    let mut acc = vec![0u16; span];
    let mut pre = vec![0u32; w + 1];
    let mut pre2 = vec![0u32; w + 1];
    let flat = (flat_var * 65_025.0 * n * n) as f32;
    let nf = n as f32;
    for cy in half..h - half {
        add_row(&mut col, &mut col2, cy + half);
        acc.iter_mut().for_each(|a| *a = 0);
        for &(dx, dy) in &offs {
            let row = &blur[(cy as i32 + dy) as usize * w..][..w];
            let start = (half as i32 + dx) as usize;
            for (a, &v) in acc.iter_mut().zip(&row[start..start + span]) {
                *a += u16::from(v);
            }
        }
        // Horizontal window sums by prefix differences (wrapping arithmetic is exact here).
        for x in 0..w {
            pre[x + 1] = pre[x].wrapping_add(col[x]);
            pre2[x + 1] = pre2[x].wrapping_add(col2[x]);
        }
        let out = &mut map.data[cy * w + half..cy * w + half + span];
        let inv_n = 1.0 / nf;
        for (i, o) in out.iter_mut().enumerate() {
            let sx = pre[i + s].wrapping_sub(pre[i]) as f32;
            let sx2 = pre2[i + s].wrapping_sub(pre2[i]) as f32;
            let nvar = nf * sx2 - sx * sx;
            let num = f32::from(acc[i]) - gbar * sx;
            // Branch-free: flat or anti-correlated windows give 0.
            let den = tnorm * (nvar.max(flat) * inv_n).sqrt();
            let v = num / den;
            *o = if nvar > flat && num > 0.0 { v } else { 0.0 };
        }
        sub_row(&mut col, &mut col2, cy - half);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_image(w: usize, h: usize, centres: &[(f64, f64)], r: f64) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let mut v = 255.0f64;
            for &(cx, cy) in centres {
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                let ink = (-((d - r) / 1.0).powi(2)).exp();
                v = v.min(255.0 * (1.0 - ink));
            }
            v.round() as u8
        })
    }

    #[test]
    fn flat_windows_score_zero() {
        let white = GrayImage::new(80, 80, 255);
        let k = RingKernel::gaussian(13.5, 1.6);
        assert_eq!(k.ncc_dark_at(&white, 10, 10, FLAT_VARIANCE), Some(0.0));
        let m = ring_map_fast(&white, 6.5, FLAT_VARIANCE);
        assert!(m.data.iter().all(|&v| v == 0.0));
        let tpl = PatchTemplate::new(&GrayImage::from_fn(5, 5, |x, y| (x * 40 + y) as u8));
        let integ = Integral::new(&white);
        assert_eq!(
            ncc_patch(&white, &integ, &tpl, 3, 3, FLAT_VARIANCE),
            Some(0.0)
        );
        // Without the guard a flat window is undefined, never 1.
        let gray = GrayImage::new(80, 80, 128);
        assert_eq!(k.ncc_dark_at(&gray, 10, 10, 0.0), Some(0.0));
    }

    #[test]
    fn ring_refines_to_subpixel_centre() {
        let img = ring_image(120, 100, &[(60.3, 47.8)], 13.5);
        let k = RingKernel::gaussian(13.5, 1.6);
        let hit = k
            .refine(&img, PointPx { x: 58.0, y: 50.0 }, FLAT_VARIANCE, 4)
            .unwrap();
        assert!((hit.centre.x - 60.3).abs() < 0.15, "{hit:?}");
        assert!((hit.centre.y - 47.8).abs() < 0.15, "{hit:?}");
        assert!(hit.score > 0.8);
    }

    #[test]
    fn fast_map_peaks_at_rings() {
        let img = ring_image(160, 90, &[(40.5, 45.5), (110.5, 44.5)], 6.5);
        let m = ring_map_fast(&img, 6.5, FLAT_VARIANCE);
        let mut peaks = local_maxima(&m, 11, 0.4);
        peaks.sort_by(|a, b| a.point.x.total_cmp(&b.point.x));
        assert_eq!(peaks.len(), 2, "{peaks:?}");
        assert!(
            (peaks[0].point.x - 40.5).abs() < 0.6 && (peaks[0].point.y - 45.5).abs() < 0.6,
            "{peaks:?}"
        );
        assert!((peaks[1].point.x - 110.5).abs() < 0.6);
    }

    #[test]
    fn patch_match_finds_offset() {
        let mut rng = crate::rng::SplitMix64::new(3);
        let img = GrayImage::from_fn(60, 50, |_, _| (rng.next_u64() >> 56) as u8);
        let tpl = PatchTemplate::new(&img.crop(17, 9, 20, 15, 255));
        let g = match_patch(&img, &tpl, 0..40, 0..35, FLAT_VARIANCE);
        let (x, y, v) = g.argmax().unwrap();
        assert_eq!((x, y), (17, 9));
        assert!((v - 1.0).abs() < 1e-9);
    }

    #[test]
    fn parabola_vertex() {
        assert!((parabolic_offset(1.0, 2.0, 1.0)).abs() < 1e-12);
        assert!((parabolic_offset(0.0, 1.0, 1.0) - 0.5).abs() < 1e-12);
    }
}
