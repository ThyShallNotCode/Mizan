//! Registration of a legacy form without fiducials (§6.10, §8.4 adapted; brief steps 2–6):
//!
//! 1. **Ring candidates** on the half-resolution page: a fast blurred-circle NCC map at one
//!    radius (`ring_map_fast`, zero-variance guard), local maxima over 1.6 R windows.
//! 2. **Pitch** from the densest cluster of near-horizontal nearest-neighbour vectors
//!    (42 ≤ dx ≤ 56 px): scale s and rotation θ. The detection radius is the member of the
//!    radius set {11.5 … 16.5} nearest `bubble radius × s`.
//! 3. **Ring detection** at full resolution: each candidate climbs to the local maximum of the
//!    exact Gaussian-ring NCC (width 1.6 px) and is refined by parabolic fits; peaks with
//!    NCC > 0.40 survive non-maximum suppression over 1.6 R. The pitch is re-measured.
//! 4. **Coarse translation** (non-periodic): the page, un-scaled and un-rotated by (s, θ), is
//!    correlated with the central 84 % of the blank form, coarse to fine — a full translation
//!    search at 1/32 (upright and upside down), 1/16 refinement, the 5 × 3 scale/angle search
//!    at 1/16, the five best at 1/8, the best at 1/4 — then an explicit **alias check**: no
//!    translation by one lattice pitch may correlate better (the brief: lattice-only alignment
//!    aliases by one pitch).
//! 5. **ICP**: nearest-neighbour matching with thresholds 12 → 8 → 6 → 4 → 3 → 3 px; two LMedS
//!    affine rounds, then weighted least-squares quadratic warps with weights 1 / max(1, r/1.5).
//!    Final matched count and RMS within 4 px.
//!
//! Failure gates raise REG-001 (no lattice, no correlation), REG-003 (alias ambiguity, matched
//! count, RMS) and REG-004 (scale, skew, anisotropy); REG-002 (INFO) logs an upside-down page.
//! A failed page measures nothing and every bubble is UNREADABLE.

use crate::config::RegistrationConfig;
use crate::exc;
use omr_imaging::{
    Affine, GrayImage, PatchTemplate, Peak, PointGrid, QuadWarp, RingHit, RingKernel, Warp,
    fit_affine_lmeds, fit_quadratic, local_maxima, match_patch, ncc_patch, resample_affine_offset,
    ring_map_fast,
};
use omr_model::{Exception, PointPx, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Registration diagnostics (always filled as far as the pipeline got).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistrationQuality {
    /// Ring candidates from the half-resolution pass.
    pub candidates: u32,
    /// Rings detected at full resolution.
    pub detections: u32,
    /// Detection radius used (px).
    pub ring_radius_px: f64,
    /// Measured option pitch (px).
    pub pitch_px: f64,
    /// Coarse correlation with the blank form (1/4 resolution).
    pub coarse_ncc: f64,
    /// Coarse NCC minus the best NCC one lattice pitch away (aliasing guard).
    pub alias_margin: f64,
    /// NCC of the chosen orientation minus the other's (upright vs upside down) at 1/32;
    /// 1.0 when only one orientation fitted the page (orientation guard, §8.3).
    #[serde(default)]
    pub orientation_margin: f64,
    /// Template bubbles matched to a ring within the final radius.
    pub matched: u32,
    /// Template bubbles.
    pub template_points: u32,
    /// RMS residual of the matched bubbles (px).
    pub rms_px: f64,
    /// Print scale (√det J at the template centre).
    pub scale: f64,
    /// Rotation (rad).
    pub rotation_rad: f64,
    /// Anisotropy s_x / s_y − 1.
    pub anisotropy: f64,
}

/// The result of registering one page.
#[derive(Clone, Debug)]
pub struct Registration {
    /// Template px → page px, if registration succeeded.
    pub warp: Option<QuadWarp>,
    /// Diagnostics.
    pub quality: RegistrationQuality,
    /// Detected rings (full resolution), ordered by (y, x).
    pub rings: Vec<RingHit>,
    /// Per template bubble: matched in the final pass.
    pub matched: Vec<bool>,
    /// Page-level registration exceptions (REG-001/003/004).
    pub exceptions: Vec<Exception>,
}

impl Registration {
    /// True if registration failed (every bubble is UNREADABLE).
    pub fn failed(&self) -> bool {
        self.warp.is_none()
    }
}

/// One pyramid level of the blank form prepared for coarse correlation.
#[derive(Clone, Debug)]
pub(crate) struct RefLevel {
    /// Box factor relative to full resolution.
    pub factor: usize,
    /// Central crop as an NCC template.
    pub crop: PatchTemplate,
    /// Crop origin in level pixels.
    pub x0: usize,
    /// Crop origin in level pixels.
    pub y0: usize,
}

/// Precomputed reference data.
#[derive(Clone, Debug)]
pub(crate) struct RefModel {
    /// Template bubble centres (reference px).
    pub points: Vec<PointPx>,
    /// Centroid of the template points (quadratic-warp normalisation centre).
    pub centre: PointPx,
    /// Nominal bubble radius.
    pub radius: f64,
    /// Blank form at 1/4, 1/8, 1/16, 1/32.
    pub levels: Vec<RefLevel>,
    /// Ring kernels for the radius set.
    pub kernels: Vec<RingKernel>,
}

impl RefModel {
    pub(crate) fn new(
        points: Vec<PointPx>,
        radius: f64,
        blank: &GrayImage,
        cfg: &RegistrationConfig,
    ) -> Self {
        let n = points.len().max(1) as f64;
        let centre = PointPx {
            x: points.iter().map(|p| p.x).sum::<f64>() / n,
            y: points.iter().map(|p| p.y).sum::<f64>() / n,
        };
        let mut levels = Vec::new();
        let mut img = blank.clone();
        let mut factor = 1;
        while factor < 32 {
            img = omr_imaging::half(&img);
            factor *= 2;
            if factor >= 4 {
                let m = cfg.coarse_crop_permille as f64 / 1000.0;
                let (w, h) = (img.width(), img.height());
                let x0 = (w as f64 * m) as usize;
                let y0 = (h as f64 * m) as usize;
                let x1 = (w as f64 * (1.0 - m)) as usize;
                let y1 = (h as f64 * (1.0 - m)) as usize;
                let crop = img.crop(x0 as isize, y0 as isize, x1 - x0, y1 - y0, 255);
                levels.push(RefLevel {
                    factor,
                    crop: PatchTemplate::new(&crop),
                    x0,
                    y0,
                });
            }
        }
        let width = cfg.ring_width_mpx as f64 / 1000.0;
        let kernels = cfg
            .radii_mpx
            .iter()
            .map(|&r| RingKernel::gaussian(r as f64 / 1000.0, width))
            .collect();
        Self {
            points,
            centre,
            radius,
            levels,
            kernels,
        }
    }
}

/// The page pyramid: full (borrowed), 1/2, 1/4, 1/8, 1/16, 1/32.
pub(crate) struct Pyramid<'a> {
    page: &'a GrayImage,
    reduced: Vec<GrayImage>,
}

impl<'a> Pyramid<'a> {
    pub(crate) fn new(page: &'a GrayImage) -> Self {
        let mut reduced: Vec<GrayImage> = Vec::with_capacity(5);
        for _ in 0..5 {
            let next = omr_imaging::half(reduced.last().unwrap_or(page));
            reduced.push(next);
        }
        Self { page, reduced }
    }

    fn at(&self, factor: usize) -> &GrayImage {
        match factor {
            1 => self.page,
            f => &self.reduced[f.trailing_zeros() as usize - 1],
        }
    }
}

fn mpx(v: u32) -> f64 {
    f64::from(v) / 1000.0
}

/// The option-pitch vector: the densest cluster of near-horizontal nearest-neighbour vectors.
pub(crate) fn pitch_vector(points: &[PointPx], cfg: &RegistrationConfig) -> Option<PointPx> {
    let (lo, hi) = (mpx(cfg.pitch_min_mpx), mpx(cfg.pitch_max_mpx));
    let dyr = f64::from(cfg.pitch_dy_permille) / 1000.0;
    let grid = PointGrid::new(points, 64.0);
    let reach = hi * (1.0 + dyr * dyr).sqrt() + 1.0;
    let mut vs = Vec::new();
    for (a, p) in points.iter().enumerate() {
        for (j, _) in grid.k_nearest_within(*p, 4, reach, Some(a)) {
            let v = PointPx {
                x: points[j].x - p.x,
                y: points[j].y - p.y,
            };
            if v.x > 0.0 && v.y.abs() < dyr * v.x && lo < v.x && v.x < hi {
                vs.push(v);
            }
        }
    }
    if vs.is_empty() {
        return None;
    }
    let mut counts: BTreeMap<(i64, i64), u32> = BTreeMap::new();
    for v in &vs {
        *counts
            .entry((v.x.round_ties_even() as i64, v.y.round_ties_even() as i64))
            .or_default() += 1;
    }
    let mut mode = (0i64, 0i64);
    let mut best = 0;
    for (k, &c) in &counts {
        if c > best {
            best = c;
            mode = *k;
        }
    }
    let (mx, my) = (mode.0 as f64, mode.1 as f64);
    let r = mpx(cfg.pitch_cluster_mpx);
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for v in &vs {
        if ((v.x - mx).powi(2) + (v.y - my).powi(2)).sqrt() < r {
            xs.push(v.x);
            ys.push(v.y);
        }
    }
    Some(PointPx {
        x: median_f64(&mut xs)?,
        y: median_f64(&mut ys)?,
    })
}

pub(crate) fn median_f64(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

fn candidates(half: &GrayImage, radius_full: f64, cfg: &RegistrationConfig) -> Vec<PointPx> {
    let r = radius_full / 2.0;
    let flat = f64::from(cfg.flat_variance_ppm) * 1e-6;
    let map = ring_map_fast(half, r, flat);
    let win = ((f64::from(cfg.nms_window_permille) / 1000.0 * r) as usize) | 1;
    let peaks: Vec<Peak> = local_maxima(&map, win, cfg.candidate_ncc_milli as f32 / 1000.0);
    peaks
        .iter()
        .map(|p| PointPx {
            x: 2.0 * p.point.x,
            y: 2.0 * p.point.y,
        })
        .collect()
}

fn detect_rings(
    page: &GrayImage,
    cands: &[PointPx],
    kernel: &RingKernel,
    cfg: &RegistrationConfig,
) -> Vec<RingHit> {
    let flat = f64::from(cfg.flat_variance_ppm) * 1e-6;
    let thr = f64::from(cfg.detect_ncc_milli) / 1000.0;
    let mut hits: Vec<RingHit> = cands
        .iter()
        .filter_map(|&c| kernel.refine(page, c, flat, 3))
        .filter(|h| h.score > thr)
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.centre.y.total_cmp(&b.centre.y))
            .then(a.centre.x.total_cmp(&b.centre.x))
    });
    let half = (((f64::from(cfg.nms_window_permille) / 1000.0 * kernel.radius()) as usize) | 1) / 2;
    let half = half as f64;
    let cell = (half + 1.0).max(1.0);
    let mut buckets: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    let mut kept: Vec<RingHit> = Vec::new();
    for h in hits {
        let (cx, cy) = (
            (h.centre.x / cell).floor() as i64,
            (h.centre.y / cell).floor() as i64,
        );
        let mut clash = false;
        'n: for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(v) = buckets.get(&(cx + dx, cy + dy)) {
                    for &k in v {
                        let o = kept[k].centre;
                        if (o.x - h.centre.x).abs() <= half && (o.y - h.centre.y).abs() <= half {
                            clash = true;
                            break 'n;
                        }
                    }
                }
            }
        }
        if !clash {
            buckets.entry((cx, cy)).or_default().push(kept.len());
            kept.push(h);
        }
    }
    kept.sort_by(|a, b| {
        a.centre
            .y
            .total_cmp(&b.centre.y)
            .then(a.centre.x.total_cmp(&b.centre.x))
    });
    kept
}

/// The coarse alignment.
#[derive(Clone, Copy, Debug)]
struct Coarse {
    affine: Affine,
    ncc: f64,
    alias_margin: f64,
    orientation_margin: f64,
    upside_down: bool,
}

/// Coarse similarity (non-periodic): the page, un-scaled and un-rotated by the pitch estimate,
/// is correlated with the central crop of the blank form: a full translation search at 1/32,
/// refinement at 1/16, the scale × angle hypotheses at 1/16, the three best at 1/8, the best at
/// 1/4 — then an explicit check that no translation by one lattice pitch correlates better.
fn coarse(
    pyr: &Pyramid,
    rm: &RefModel,
    s0: f64,
    th0: f64,
    cfg: &RegistrationConfig,
) -> Option<Coarse> {
    let flat = f64::from(cfg.flat_variance_ppm) * 1e-6;
    let level = |f: usize| rm.levels.iter().find(|l| l.factor == f);
    let (l32, l16, l8, l4) = (level(32)?, level(16)?, level(8)?, level(4)?);
    // The page resampled into the reference frame (M = s·R(θ)) over the shifts
    // e ∈ [ex0, ex0 + nx) × [ey0, ey0 + ny) (level px).
    let resampled = |lv: &RefLevel, m: &Affine, ex0: isize, ey0: isize, nx: usize, ny: usize| {
        resample_affine_offset(
            pyr.at(lv.factor),
            lv.crop.width() + nx - 1,
            lv.crop.height() + ny - 1,
            m,
            lv.x0 as f64 + ex0 as f64,
            lv.y0 as f64 + ey0 as f64,
            255,
        )
    };
    let search = |lv: &RefLevel, m: &Affine, ex0: isize, ey0: isize, nx: usize, ny: usize| {
        let o = resampled(lv, m, ex0, ey0, nx, ny);
        let g = match_patch(&o, &lv.crop, 0..nx as isize, 0..ny as isize, flat);
        g.argmax().map(|(i, j, v)| (ex0 + i, ey0 + j, v))
    };
    let lin = |s: f64, th: f64| Affine::similarity(s, th, 0.0, 0.0);
    // 1/32: full translation search at the pitch hypothesis, upright and upside down (the
    // lattice pitch cannot tell a page fed the other way round, §8.3, REG-002).
    let p32 = pyr.at(32);
    let pad = (f64::from(cfg.coarse_pad_px) / 32.0).ceil() as isize;
    let (cw, ch) = (l32.crop.width() as isize, l32.crop.height() as isize);
    let full = |th: f64| {
        let m = lin(s0, th);
        // Page extent in the un-rotated frame (corners mapped back).
        let inv = m.inverse()?;
        let (pw, ph) = (p32.width() as f64, p32.height() as f64);
        let corners =
            [(0.0, 0.0), (pw, 0.0), (0.0, ph), (pw, ph)].map(|(x, y)| inv.map(PointPx { x, y }));
        let lo_x = corners
            .iter()
            .map(|c| c.x)
            .fold(f64::INFINITY, f64::min)
            .floor() as isize;
        let hi_x = corners
            .iter()
            .map(|c| c.x)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil() as isize;
        let lo_y = corners
            .iter()
            .map(|c| c.y)
            .fold(f64::INFINITY, f64::min)
            .floor() as isize;
        let hi_y = corners
            .iter()
            .map(|c| c.y)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil() as isize;
        let ex0 = lo_x - pad - l32.x0 as isize;
        let ex1 = hi_x + pad - (l32.x0 as isize + cw);
        let ey0 = lo_y - pad - l32.y0 as isize;
        let ey1 = hi_y + pad - (l32.y0 as isize + ch);
        if ex1 < ex0 || ey1 < ey0 {
            return None;
        }
        search(
            l32,
            &m,
            ex0,
            ey0,
            (ex1 - ex0 + 1) as usize,
            (ey1 - ey0 + 1) as usize,
        )
    };
    let upright = full(th0);
    let flipped = full(th0 + std::f64::consts::PI);
    let orientation_margin = match (upright, flipped) {
        (Some(u), Some(f)) => (u.2 - f.2).abs(),
        _ => 1.0,
    };
    let (th0, (e32x, e32y, _), upside_down) = match (upright, flipped) {
        (Some(u), Some(f)) if f.2 > u.2 => (th0 + std::f64::consts::PI, f, true),
        (Some(u), _) => (th0, u, false),
        (None, Some(f)) => (th0 + std::f64::consts::PI, f, true),
        (None, None) => return None,
    };
    let m0 = lin(s0, th0);
    // 1/16: refine, then the page point of the crop centre (the anchor, full px).
    let (e16x, e16y, _) = search(l16, &m0, 2 * e32x - 3, 2 * e32y - 3, 7, 7)?;
    let rc = PointPx {
        x: (l16.x0 as f64 + l16.crop.width() as f64 / 2.0) * 16.0,
        y: (l16.y0 as f64 + l16.crop.height() as f64 / 2.0) * 16.0,
    };
    let anchor = m0.map(PointPx {
        x: rc.x + e16x as f64 * 16.0,
        y: rc.y + e16y as f64 * 16.0,
    });
    // Scale × angle hypotheses at 1/16, each centred on the anchor.
    let mut hyps: Vec<(f64, f64, f64, isize, isize)> = Vec::new();
    for &ds in &cfg.coarse_scale_permille {
        for &da in &cfg.coarse_angle_urad {
            let s = s0 * f64::from(ds) / 1000.0;
            let th = th0 + f64::from(da) * 1e-6;
            let m = lin(s, th);
            let u = m.inverse()?.map(anchor);
            let ex = ((u.x - rc.x) / 16.0).round() as isize;
            let ey = ((u.y - rc.y) / 16.0).round() as isize;
            if let Some((x, y, v)) = search(l16, &m, ex - 2, ey - 2, 5, 5) {
                hyps.push((v, s, th, x, y));
            }
        }
    }
    hyps.sort_by(|a, b| b.0.total_cmp(&a.0));
    // 1/8: the five best.
    let mut best8: Option<(f64, f64, f64, isize, isize)> = None;
    for &(_, s, th, x, y) in hyps.iter().take(5) {
        if let Some((ex, ey, v)) = search(l8, &lin(s, th), 2 * x - 2, 2 * y - 2, 5, 5)
            && best8.is_none_or(|b| v > b.0)
        {
            best8 = Some((v, s, th, ex, ey));
        }
    }
    let (_, s, th, x8, y8) = best8?;
    let m = lin(s, th);
    // 1/4: ±2 around the 1/8 solution and the one-pitch alias check, on one resampled window.
    let ax = (mpx(cfg.alias_pitch_mpx[0]) / 4.0).round() as isize;
    let ay = (mpx(cfg.alias_pitch_mpx[1]) / 4.0).round() as isize;
    let rx = 2 * ax + 2;
    let ry = 2 * ay + 2;
    let (cx0, cy0) = (2 * x8, 2 * y8);
    let o = resampled(
        l4,
        &m,
        cx0 - rx,
        cy0 - ry,
        (2 * rx + 1) as usize,
        (2 * ry + 1) as usize,
    );
    let integral = omr_imaging::Integral::new(&o);
    let at = |ex: isize, ey: isize| {
        ncc_patch(
            &o,
            &integral,
            &l4.crop,
            ex - (cx0 - rx),
            ey - (cy0 - ry),
            flat,
        )
        .unwrap_or(f64::NEG_INFINITY)
    };
    let local = |cx: isize, cy: isize| {
        let mut best = (cx, cy, f64::NEG_INFINITY);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let v = at(cx + dx, cy + dy);
                if v > best.2 {
                    best = (cx + dx, cy + dy, v);
                }
            }
        }
        best
    };
    let aliases = |cx: isize, cy: isize| {
        let mut best = (cx, cy, f64::NEG_INFINITY);
        for ky in -1..=1isize {
            for kx in -1..=1isize {
                if (kx, ky) != (0, 0) {
                    let v = at(cx + kx * ax, cy + ky * ay);
                    if v > best.2 {
                        best = (cx + kx * ax, cy + ky * ay, v);
                    }
                }
            }
        }
        best
    };
    let mut cur = local(cx0, cy0);
    let mut alt = aliases(cur.0, cur.1);
    if alt.2 > cur.2 {
        // A one-pitch translation correlates better: move there (within the evaluated window).
        let moved = local(
            alt.0.clamp(cx0 - ax, cx0 + ax),
            alt.1.clamp(cy0 - ay, cy0 + ay),
        );
        if moved.2 > cur.2 {
            cur = moved;
            alt = aliases(cur.0, cur.1);
        }
    }
    let t = m.map(PointPx {
        x: cur.0 as f64 * 4.0,
        y: cur.1 as f64 * 4.0,
    });
    let [a, b, _, c, d, _] = m.m;
    Some(Coarse {
        affine: Affine::new(a, b, t.x, c, d, t.y),
        ncc: cur.2,
        alias_margin: cur.2 - alt.2,
        orientation_margin,
        upside_down,
    })
}

fn reg_exc(code: &str, sev: Severity, sub: Option<&str>, detail: String) -> Exception {
    let e = exc::page(code, sev).with_detail(detail);
    match sub {
        Some(s) => e.with_sub_reason(s),
        None => e,
    }
}

/// Registers a page against the template.
pub(crate) fn register(page: &GrayImage, rm: &RefModel, cfg: &RegistrationConfig) -> Registration {
    let pyr = Pyramid::new(page);
    register_with(page, &pyr, rm, cfg)
}

pub(crate) fn register_with(
    page: &GrayImage,
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
) -> Registration {
    let n_t = rm.points.len();
    let mut q = RegistrationQuality {
        template_points: n_t as u32,
        ..Default::default()
    };
    let fail = |q: RegistrationQuality, rings: Vec<RingHit>, e: Exception| Registration {
        warp: None,
        quality: q,
        rings,
        matched: vec![false; n_t],
        exceptions: vec![e],
    };
    // 1. Candidates.
    let half = pyr.at(2);
    let mut cands = candidates(half, mpx(cfg.candidate_radius_mpx), cfg);
    if cands.len() < cfg.min_rings as usize {
        for &r in &cfg.fallback_candidate_radii_mpx {
            let c = candidates(half, mpx(r), cfg);
            if c.len() > cands.len() {
                cands = c;
            }
        }
    }
    q.candidates = cands.len() as u32;
    let Some(v0) = pitch_vector(&cands, cfg) else {
        return fail(
            q,
            Vec::new(),
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                "no bubble lattice found (no option pitch)".into(),
            ),
        );
    };
    let tp = mpx(cfg.template_pitch_mpx);
    let s_cand = (v0.x * v0.x + v0.y * v0.y).sqrt() / tp;
    // 2. Detection radius nearest R0 · s.
    let want = rm.radius * s_cand;
    let Some(kernel) = rm.kernels.iter().min_by(|a, b| {
        (a.radius() - want)
            .abs()
            .total_cmp(&(b.radius() - want).abs())
    }) else {
        return fail(
            q,
            Vec::new(),
            reg_exc("REG-001", Severity::Blocker, None, "no ring kernels".into()),
        );
    };
    q.ring_radius_px = kernel.radius();
    // 3. Full-resolution rings.
    let rings = detect_rings(page, &cands, kernel, cfg);
    q.detections = rings.len() as u32;
    if rings.len() < cfg.min_rings as usize {
        let detail = format!("only {} bubble rings detected", q.detections);
        return fail(
            q,
            rings,
            reg_exc("REG-001", Severity::Blocker, None, detail),
        );
    }
    let pts: Vec<PointPx> = rings.iter().map(|r| r.centre).collect();
    let v = pitch_vector(&pts, cfg).unwrap_or(v0);
    let pitch = (v.x * v.x + v.y * v.y).sqrt();
    q.pitch_px = pitch;
    let (s0, th0) = (pitch / tp, v.y.atan2(v.x));
    // 4. Coarse translation.
    let co = coarse(pyr, rm, s0, th0, cfg);
    let Some(Coarse {
        affine: a0,
        ncc,
        alias_margin,
        orientation_margin,
        upside_down,
    }) = co
    else {
        return fail(
            q,
            rings,
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                "coarse alignment with the blank form failed".into(),
            ),
        );
    };
    q.coarse_ncc = ncc;
    q.alias_margin = alias_margin;
    q.orientation_margin = orientation_margin;
    if ncc < f64::from(cfg.coarse_min_ncc_milli) / 1000.0 {
        return fail(
            q,
            rings,
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                format!("page does not correlate with the blank form (NCC {ncc:.3})"),
            ),
        );
    }
    if orientation_margin < f64::from(cfg.orientation_min_margin_milli) / 1000.0 {
        let detail = format!(
            "orientation undecided: the page correlates with the blank form about as well \
             upright as upside down (NCC margin {orientation_margin:.3}); rescan it upright"
        );
        return fail(
            q,
            rings,
            reg_exc("REG-001", Severity::Blocker, None, detail),
        );
    }
    if alias_margin < f64::from(cfg.alias_min_margin_milli) / 1000.0 {
        let detail =
            format!("alignment ambiguous by one lattice pitch (NCC margin {alias_margin:.3})");
        return fail(q, rings, reg_exc("REG-003", Severity::Error, None, detail));
    }
    // 5. ICP.
    let grid = PointGrid::new(&pts, 16.0);
    let norm = f64::from(cfg.quad_norm_px);
    let mut warp = QuadWarp::from_affine(&a0, rm.centre, norm);
    let huber = mpx(cfg.huber_mpx);
    let mut src = Vec::with_capacity(n_t);
    let mut dst = Vec::with_capacity(n_t);
    let mut wts = Vec::with_capacity(n_t);
    for (it, &thr) in cfg.icp_thresholds_mpx.iter().enumerate() {
        src.clear();
        dst.clear();
        wts.clear();
        for t in &rm.points {
            if let Some((k, d)) = grid.nearest_within(warp.map(*t), mpx(thr)) {
                src.push(*t);
                dst.push(pts[k]);
                wts.push(1.0 / (d / huber).max(1.0));
            }
        }
        if src.len() < cfg.icp_min_matches as usize {
            break;
        }
        if (it as u32) < cfg.icp_affine_rounds {
            if let Some(f) = fit_affine_lmeds(
                &src,
                &dst,
                cfg.lmeds_iterations as usize,
                cfg.lmeds_seed.wrapping_add(it as u64),
            ) {
                warp = QuadWarp::from_affine(&f.model, rm.centre, norm);
            }
        } else if let Some(w) = fit_quadratic(&src, &dst, Some(&wts), rm.centre, norm) {
            warp = w;
        }
    }
    let mut matched = vec![false; n_t];
    let mut ss = 0.0;
    let mut nm = 0u32;
    for (j, t) in rm.points.iter().enumerate() {
        if let Some((_, d)) = grid.nearest_within(warp.map(*t), mpx(cfg.final_match_mpx)) {
            matched[j] = true;
            ss += d * d;
            nm += 1;
        }
    }
    q.matched = nm;
    q.rms_px = if nm > 0 {
        (ss / f64::from(nm)).sqrt()
    } else {
        f64::INFINITY
    };
    let j = warp.jacobian(rm.centre);
    let dec = Affine::new(j[0][0], j[0][1], 0.0, j[1][0], j[1][1], 0.0).decompose();
    q.scale = dec.scale;
    q.rotation_rad = dec.rotation;
    q.anisotropy = dec.anisotropy;
    // Gates.
    let mut exceptions = Vec::new();
    let frac = u64::from(nm) * 1000;
    let rms_mpx = q.rms_px * 1000.0;
    if frac < u64::from(cfg.matched_error_permille) * n_t as u64
        || rms_mpx > f64::from(cfg.rms_error_mpx)
    {
        return fail(
            q.clone(),
            rings,
            reg_exc(
                "REG-003",
                Severity::Error,
                None,
                format!(
                    "registration failed: {} of {} bubbles matched, RMS {:.2} px",
                    nm, n_t, q.rms_px
                ),
            ),
        );
    }
    let sc = q.scale * 1000.0;
    if sc < f64::from(cfg.scale_warn_permille[0]) || sc > f64::from(cfg.scale_warn_permille[1]) {
        return fail(
            q.clone(),
            rings,
            reg_exc(
                "REG-004",
                Severity::Error,
                Some("scale"),
                format!(
                    "print scale {:.3} outside 0.80–1.10 (wrong template or photocopy)",
                    q.scale
                ),
            ),
        );
    }
    if frac < u64::from(cfg.matched_warn_permille) * n_t as u64
        || rms_mpx > f64::from(cfg.rms_warn_mpx)
    {
        exceptions.push(reg_exc(
            "REG-003",
            Severity::Warn,
            None,
            format!("{} of {} bubbles matched, RMS {:.2} px", nm, n_t, q.rms_px),
        ));
    }
    if sc < f64::from(cfg.scale_silent_permille[0]) || sc > f64::from(cfg.scale_silent_permille[1])
    {
        exceptions.push(reg_exc(
            "REG-004",
            Severity::Warn,
            Some("scale"),
            format!("print scale {:.3}; print at 100 %", q.scale),
        ));
    }
    if upside_down {
        exceptions.push(reg_exc(
            "REG-002",
            Severity::Info,
            None,
            "page fed upside down; read rotated by 180°".into(),
        ));
    }
    let skew = q.rotation_rad
        - if upside_down {
            std::f64::consts::PI
        } else {
            0.0
        };
    let skew =
        (skew + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI) - std::f64::consts::PI;
    if skew.abs() * 1e6 > f64::from(cfg.skew_warn_urad) {
        exceptions.push(reg_exc(
            "REG-004",
            Severity::Warn,
            Some("skew"),
            format!("skew {:.2}°", skew.to_degrees()),
        ));
    }
    if q.anisotropy.abs() * 1000.0 > f64::from(cfg.anisotropy_warn_permille) {
        exceptions.push(reg_exc(
            "REG-004",
            Severity::Warn,
            Some("anisotropy"),
            format!("anisotropy {:.2} %", q.anisotropy * 100.0),
        ));
    }
    Registration {
        warp: Some(warp),
        quality: q,
        rings,
        matched,
        exceptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_of_a_lattice() {
        let mut pts = Vec::new();
        let (s, th): (f64, f64) = (0.93, 0.02);
        for j in 0..10 {
            for i in 0..12 {
                let (x, y) = (50.0 * i as f64, 41.5 * j as f64);
                pts.push(PointPx {
                    x: 300.0 + s * (x * th.cos() - y * th.sin()),
                    y: 200.0 + s * (x * th.sin() + y * th.cos()),
                });
            }
        }
        let v = pitch_vector(&pts, &RegistrationConfig::default()).unwrap();
        let p = (v.x * v.x + v.y * v.y).sqrt();
        assert!((p / 50.0 - s).abs() < 1e-3, "{p}");
        assert!((v.y.atan2(v.x) - th).abs() < 2e-3);
    }
}
