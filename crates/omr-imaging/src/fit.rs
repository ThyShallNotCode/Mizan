//! Robust and weighted least-squares fits of warps to point correspondences (§8.4).
//!
//! - [`fit_affine`]: weighted least squares (6 parameters).
//! - [`fit_affine_lmeds`]: least median of squares over seeded 3-point samples, then least
//!   squares on the inliers (the OpenCV `LMEDS` recipe).
//! - [`fit_quadratic`]: weighted least squares for the 12-parameter [`QuadWarp`].
//! - [`fit_homography`]: normalised DLT; [`ransac_homography`]: seeded RANSAC with adaptive
//!   stopping, then DLT on the inliers.
//!
//! Everything is deterministic: fixed loop order, seeded [`SplitMix64`] sampling, no FMA.

use crate::linalg::{solve, sym_eigen};
use crate::rng::SplitMix64;
use crate::warp::{Affine, Homography, QuadWarp, Warp};
use omr_model::PointPx;

fn weight(w: Option<&[f64]>, i: usize) -> f64 {
    w.map_or(1.0, |w| w[i])
}

/// Weighted least-squares affine `dst ≈ A(src)`; needs ≥ 3 non-collinear points.
pub fn fit_affine(src: &[PointPx], dst: &[PointPx], w: Option<&[f64]>) -> Option<Affine> {
    let n = src.len().min(dst.len());
    if n < 3 {
        return None;
    }
    // Centre for conditioning.
    let (mut cx, mut cy, mut sw) = (0.0, 0.0, 0.0);
    for (i, p) in src.iter().enumerate().take(n) {
        let wi = weight(w, i);
        cx += wi * p.x;
        cy += wi * p.y;
        sw += wi;
    }
    if sw <= 0.0 {
        return None;
    }
    cx /= sw;
    cy /= sw;
    let mut ata = [0.0; 9];
    let mut bx = [0.0; 3];
    let mut by = [0.0; 3];
    for i in 0..n {
        let wi = weight(w, i);
        let f = [1.0, src[i].x - cx, src[i].y - cy];
        for r in 0..3 {
            for c in 0..3 {
                ata[r * 3 + c] += wi * f[r] * f[c];
            }
            bx[r] += wi * f[r] * dst[i].x;
            by[r] += wi * f[r] * dst[i].y;
        }
    }
    let mut a2 = ata;
    solve(&mut ata, &mut bx, 3)?;
    solve(&mut a2, &mut by, 3)?;
    // x' = bx0 + bx1 (x − cx) + bx2 (y − cy)
    Some(Affine::new(
        bx[1],
        bx[2],
        bx[0] - bx[1] * cx - bx[2] * cy,
        by[1],
        by[2],
        by[0] - by[1] * cx - by[2] * cy,
    ))
}

/// Result of a robust fit.
#[derive(Clone, Debug, PartialEq)]
pub struct RobustFit<T> {
    /// The refined model.
    pub model: T,
    /// Per-correspondence inlier flags.
    pub inliers: Vec<bool>,
    /// The robust scale statistic (LMedS: best median squared residual; RANSAC: the threshold).
    pub stat: f64,
}

fn sq_res<W: Warp>(m: &W, s: PointPx, d: PointPx) -> f64 {
    let p = m.map(s);
    (p.x - d.x) * (p.x - d.x) + (p.y - d.y) * (p.y - d.y)
}

fn tri_area(a: PointPx, b: PointPx, c: PointPx) -> f64 {
    ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs() * 0.5
}

/// Least-median-of-squares affine with `iterations` seeded 3-point samples, refined by least
/// squares on the inliers (`r² ≤ (2.5 · 1.4826 · (1 + 5/(n − 3)) · √median)²`).
pub fn fit_affine_lmeds(
    src: &[PointPx],
    dst: &[PointPx],
    iterations: usize,
    seed: u64,
) -> Option<RobustFit<Affine>> {
    let n = src.len().min(dst.len());
    if n < 3 {
        return None;
    }
    let mut rng = SplitMix64::new(seed);
    let mut idx = Vec::with_capacity(3);
    let mut res = vec![0.0; n];
    let mut best: Option<(f64, Affine)> = None;
    let min_area = 1.0;
    for _ in 0..iterations.max(1) {
        rng.distinct(n, 3, &mut idx);
        let (s, d): (Vec<PointPx>, Vec<PointPx>) = idx.iter().map(|&i| (src[i], dst[i])).unzip();
        if tri_area(s[0], s[1], s[2]) < min_area {
            continue;
        }
        let Some(a) = fit_affine(&s, &d, None) else {
            continue;
        };
        for i in 0..n {
            res[i] = sq_res(&a, src[i], dst[i]);
        }
        let mid = n / 2;
        let (_, med, _) = res.select_nth_unstable_by(mid, f64::total_cmp);
        let med = *med;
        if best.as_ref().is_none_or(|(m, _)| med < *m) {
            best = Some((med, a));
        }
    }
    let (med, a) = best?;
    let sigma = 2.5 * 1.4826 * (1.0 + 5.0 / (n as f64 - 3.0).max(1.0)) * med.sqrt();
    let thr = (sigma * sigma).max(1e-12);
    let inliers: Vec<bool> = (0..n).map(|i| sq_res(&a, src[i], dst[i]) <= thr).collect();
    let (si, di): (Vec<PointPx>, Vec<PointPx>) = (0..n)
        .filter(|&i| inliers[i])
        .map(|i| (src[i], dst[i]))
        .unzip();
    let model = if si.len() >= 3 {
        fit_affine(&si, &di, None).unwrap_or(a)
    } else {
        a
    };
    Some(RobustFit {
        model,
        inliers,
        stat: med,
    })
}

/// Weighted least-squares quadratic polynomial warp with the given normalisation; needs ≥ 6
/// well-spread points.
pub fn fit_quadratic(
    src: &[PointPx],
    dst: &[PointPx],
    w: Option<&[f64]>,
    centre: PointPx,
    norm: f64,
) -> Option<QuadWarp> {
    let n = src.len().min(dst.len());
    if n < 6 {
        return None;
    }
    let proto = QuadWarp {
        centre,
        norm,
        cx: [0.0; 6],
        cy: [0.0; 6],
    };
    let mut ata = [0.0; 36];
    let mut bx = [0.0; 6];
    let mut by = [0.0; 6];
    for i in 0..n {
        let wi = weight(w, i);
        let f = proto.basis(src[i]);
        for r in 0..6 {
            let wf = wi * f[r];
            for c in 0..6 {
                ata[r * 6 + c] += wf * f[c];
            }
            bx[r] += wf * dst[i].x;
            by[r] += wf * dst[i].y;
        }
    }
    let mut a2 = ata;
    solve(&mut ata, &mut bx, 6)?;
    solve(&mut a2, &mut by, 6)?;
    Some(QuadWarp {
        centre,
        norm,
        cx: bx,
        cy: by,
    })
}

/// Hartley normalisation: translate to the centroid, scale mean distance to √2.
fn normaliser(p: &[PointPx]) -> Affine {
    let n = p.len() as f64;
    let (mx, my) = p
        .iter()
        .fold((0.0, 0.0), |(a, b), q| (a + q.x / n, b + q.y / n));
    let md = p
        .iter()
        .map(|q| ((q.x - mx).powi(2) + (q.y - my).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    let s = if md > 0.0 {
        std::f64::consts::SQRT_2 / md
    } else {
        1.0
    };
    Affine::new(s, 0.0, -s * mx, 0.0, s, -s * my)
}

/// Normalised DLT homography `dst ≈ H(src)` (least squares over all points; ≥ 4 points).
pub fn fit_homography(src: &[PointPx], dst: &[PointPx]) -> Option<Homography> {
    let n = src.len().min(dst.len());
    if n < 4 {
        return None;
    }
    let ts = normaliser(&src[..n]);
    let td = normaliser(&dst[..n]);
    let mut ata = [0.0; 81];
    for i in 0..n {
        let s = ts.map(src[i]);
        let d = td.map(dst[i]);
        let r1 = [-s.x, -s.y, -1.0, 0.0, 0.0, 0.0, d.x * s.x, d.x * s.y, d.x];
        let r2 = [0.0, 0.0, 0.0, -s.x, -s.y, -1.0, d.y * s.x, d.y * s.y, d.y];
        for r in 0..9 {
            for c in 0..9 {
                ata[r * 9 + c] += r1[r] * r1[c] + r2[r] * r2[c];
            }
        }
    }
    let (vals, vecs) = sym_eigen(&ata, 9);
    let k = (0..9).min_by(|&a, &b| vals[a].total_cmp(&vals[b]))?;
    let hn: Vec<f64> = (0..9).map(|r| vecs[r * 9 + k]).collect();
    // H = Td⁻¹ · Hn · Ts
    let tdi = Homography::from_affine(&td.inverse()?);
    let tsh = Homography::from_affine(&ts);
    let m = mul3(&tdi.h, &mul3(&hn.try_into().ok()?, &tsh.h));
    let s = m[8];
    if s.abs() < 1e-15 || !s.is_finite() {
        return None;
    }
    let mut h = [0.0; 9];
    for (o, v) in h.iter_mut().zip(m) {
        *o = v / s;
    }
    Some(Homography { h })
}

fn mul3(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    o
}

/// Seeded RANSAC homography: 4-point samples, inliers within `threshold` px, adaptive stopping
/// at `confidence`, at most `max_iterations`; the result is refitted on all inliers.
pub fn ransac_homography(
    src: &[PointPx],
    dst: &[PointPx],
    threshold: f64,
    confidence: f64,
    max_iterations: usize,
    seed: u64,
) -> Option<RobustFit<Homography>> {
    let n = src.len().min(dst.len());
    if n < 4 {
        return None;
    }
    let thr2 = threshold * threshold;
    let mut rng = SplitMix64::new(seed);
    let mut idx = Vec::with_capacity(4);
    let mut best: Option<(usize, Homography)> = None;
    let mut needed = max_iterations;
    let mut it = 0;
    while it < needed.min(max_iterations) {
        it += 1;
        rng.distinct(n, 4, &mut idx);
        let (s, d): (Vec<PointPx>, Vec<PointPx>) = idx.iter().map(|&i| (src[i], dst[i])).unzip();
        if tri_area(s[0], s[1], s[2]) < 1.0
            || tri_area(s[0], s[1], s[3]) < 1.0
            || tri_area(s[0], s[2], s[3]) < 1.0
            || tri_area(s[1], s[2], s[3]) < 1.0
        {
            continue;
        }
        let Some(h) = fit_homography(&s, &d) else {
            continue;
        };
        let count = (0..n)
            .filter(|&i| sq_res(&h, src[i], dst[i]) <= thr2)
            .count();
        if best.as_ref().is_none_or(|(c, _)| count > *c) {
            best = Some((count, h));
            let ratio = count as f64 / n as f64;
            let p_good = ratio.powi(4);
            if p_good >= 1.0 {
                needed = it;
            } else if p_good > 0.0 {
                let k = (1.0 - confidence).ln() / (1.0 - p_good).ln();
                needed = (k.ceil() as usize).max(it);
            }
        }
    }
    let (_, h) = best?;
    let inliers: Vec<bool> = (0..n).map(|i| sq_res(&h, src[i], dst[i]) <= thr2).collect();
    let (si, di): (Vec<PointPx>, Vec<PointPx>) = (0..n)
        .filter(|&i| inliers[i])
        .map(|i| (src[i], dst[i]))
        .unzip();
    let model = fit_homography(&si, &di).unwrap_or(h);
    let inliers: Vec<bool> = (0..n)
        .map(|i| sq_res(&model, src[i], dst[i]) <= thr2)
        .collect();
    Some(RobustFit {
        model,
        inliers,
        stat: threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<PointPx> {
        let mut v = Vec::new();
        for j in 0..12 {
            for i in 0..16 {
                v.push(PointPx {
                    x: 100.0 + 120.0 * i as f64,
                    y: 80.0 + 110.0 * j as f64,
                });
            }
        }
        v
    }

    #[test]
    fn affine_exact() {
        let a = Affine::new(0.91, 0.03, 40.0, -0.02, 0.92, -15.0);
        let s = grid();
        let d: Vec<PointPx> = s.iter().map(|&p| a.map(p)).collect();
        let f = fit_affine(&s, &d, None).unwrap();
        for k in 0..6 {
            assert!((f.m[k] - a.m[k]).abs() < 1e-9, "{k}");
        }
    }

    #[test]
    fn lmeds_rejects_outliers() {
        let a = Affine::similarity(1.004, 0.03, 12.0, 7.0);
        let s = grid();
        let mut d: Vec<PointPx> = s.iter().map(|&p| a.map(p)).collect();
        for i in (0..d.len()).step_by(4) {
            d[i].x += 25.0 + i as f64 % 13.0;
            d[i].y -= 18.0;
        }
        let f = fit_affine_lmeds(&s, &d, 200, 1).unwrap();
        for k in 0..6 {
            assert!((f.model.m[k] - a.m[k]).abs() < 1e-6, "{k}");
        }
        assert_eq!(f.inliers.iter().filter(|&&b| !b).count(), d.len() / 4);
    }

    #[test]
    fn quadratic_recovers_known_warp() {
        let truth = QuadWarp {
            centre: PointPx {
                x: 1100.0,
                y: 700.0,
            },
            norm: 1000.0,
            cx: [1090.0, 905.0, 20.0, 4.0, -2.5, 1.5],
            cy: [720.0, -15.0, 910.0, -1.0, 3.0, 2.0],
        };
        let s = grid();
        let d: Vec<PointPx> = s.iter().map(|&p| truth.map(p)).collect();
        let f = fit_quadratic(&s, &d, None, truth.centre, truth.norm).unwrap();
        for k in 0..6 {
            assert!((f.cx[k] - truth.cx[k]).abs() < 1e-6);
            assert!((f.cy[k] - truth.cy[k]).abs() < 1e-6);
        }
    }

    #[test]
    fn homography_dlt_and_ransac() {
        let h = Homography {
            h: [0.98, 0.05, 30.0, -0.03, 1.02, -12.0, 2e-5, -1e-5, 1.0],
        };
        let s = grid();
        let mut d: Vec<PointPx> = s.iter().map(|&p| h.map(p)).collect();
        let f = fit_homography(&s, &d).unwrap();
        for p in &s {
            let (a, b) = (f.map(*p), h.map(*p));
            assert!((a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6);
        }
        for i in (0..d.len()).step_by(5) {
            d[i].x += 40.0;
        }
        let r = ransac_homography(&s, &d, 2.0, 0.999, 2000, 9).unwrap();
        assert_eq!(
            r.inliers.iter().filter(|&&b| !b).count(),
            d.len().div_ceil(5)
        );
        for p in &s {
            let (a, b) = (r.model.map(*p), h.map(*p));
            assert!((a.x - b.x).abs() < 1e-5 && (a.y - b.y).abs() < 1e-5);
        }
    }
}
