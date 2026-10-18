//! Stage A (§8.4): the global template-mm → image-px map from the refined tag corners.
//!
//! The 16 black-square corners (4 per tag) are matched to the layout by tag ID (§6.4: IDs
//! 4k … 4k+3 sit at TL, TR, BR, BL), so any page rotation is absorbed by the fit itself; the
//! corners within a tag are matched by the best cyclic shift against an initial similarity
//! from the tag centres (the detector's corner order is not trusted). A mirrored page (§8.3
//! step 2) is fitted in the mirrored frame and composed with the mirror, so the image is never
//! resampled. The fit is a seeded RANSAC homography (`omr_imaging::ransac_homography`), then
//! normalised DLT on the inliers; an affine competes by AICc (2N observations, k = 6 vs 8) and
//! a tag whose residual stands out is dropped once (leave-one-out, §8.4 "Outliers").

use super::config::StageAConfig;
use super::refine::{Refined, refine_quad};
use super::tags::TagHit;
use crate::exc;
use omr_forms::PageGeometry;
use omr_imaging::{
    Affine, GrayImage, Homography, Warp, fit_affine, fit_homography, ransac_homography,
};
use omr_model::{Exception, PointPx, Severity};
use serde::{Deserialize, Serialize};

/// The model family chosen by AICc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitModel {
    /// 6 parameters.
    Affine,
    /// 8 parameters.
    Homography,
}

/// Stage A result and diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageA {
    /// Template millimetres → original image pixels.
    pub h: Homography,
    /// Chosen model.
    pub model: FitModel,
    /// Corner indices (0 TL, 1 TR, 2 BR, 3 BL) of the tags used in the final fit.
    pub tags_used: Vec<u8>,
    /// Tags whose corners were refined to sub-pixel.
    pub tags_refined: u8,
    /// RMS corner residual (px).
    pub rms_px: f64,
    /// RMS corner residual on the page (µm).
    pub rms_um: f64,
    /// Largest corner residual (px).
    pub max_px: f64,
    /// The page is mirrored (REG-005).
    pub mirrored: bool,
    /// Clockwise quarter turns of the page in the image (REG-002 when ≠ 0).
    pub quarter_turns: u8,
    /// Skew after removing the quarter turns (rad).
    pub skew_rad: f64,
    /// Effective resolution, pixels per millimetre (√(s_x·s_y)).
    pub px_per_mm: f64,
    /// s_x / s_y − 1.
    pub anisotropy: f64,
    /// Tag black level K (median over the tags' border modules).
    pub black: u8,
    /// Tag white level (median over the tags' white borders).
    pub white: u8,
}

impl StageA {
    /// Effective DPI (§8.2: 25.4 × px per mm).
    pub fn dpi_eff(&self) -> f64 {
        25.4 * self.px_per_mm
    }
}

/// Why Stage A failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageAFailure {
    /// Fewer than two usable tags.
    TooFewTags,
    /// The fit is degenerate.
    Degenerate,
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

fn centre(pts: &[PointPx]) -> PointPx {
    let n = pts.len().max(1) as f64;
    let (x, y) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    PointPx { x: x / n, y: y / n }
}

/// Least-squares similarity `dst ≈ s·R·src + t` (no reflection); needs ≥ 2 points.
fn fit_similarity(src: &[PointPx], dst: &[PointPx]) -> Option<Affine> {
    if src.len() < 2 || src.len() != dst.len() {
        return None;
    }
    let (cs, cd) = (centre(src), centre(dst));
    let (mut num_a, mut num_b, mut den) = (0.0, 0.0, 0.0);
    for (s, d) in src.iter().zip(dst) {
        let (x, y) = (s.x - cs.x, s.y - cs.y);
        let (u, v) = (d.x - cd.x, d.y - cd.y);
        num_a += x * u + y * v;
        num_b += x * v - y * u;
        den += x * x + y * y;
    }
    if den <= 0.0 {
        return None;
    }
    let (a, b) = (num_a / den, num_b / den);
    Some(Affine::new(
        a,
        -b,
        cd.x - a * cs.x + b * cs.y,
        b,
        a,
        cd.y - b * cs.x - a * cs.y,
    ))
}

struct TagCorr {
    corner: u8,
    mm: [PointPx; 4],
    px: [PointPx; 4],
}

fn residuals(h: &Homography, corrs: &[TagCorr]) -> Vec<f64> {
    corrs
        .iter()
        .flat_map(|t| {
            t.mm.iter().zip(&t.px).map(|(m, p)| {
                let q = h.map(*m);
                (q.x - p.x).hypot(q.y - p.y)
            })
        })
        .collect()
}

fn aicc(rss: f64, n_points: usize, k: usize) -> f64 {
    let n2 = 2.0 * n_points as f64;
    let k = k as f64;
    let denom = n2 - k - 1.0;
    if denom <= 0.0 {
        return f64::INFINITY;
    }
    n2 * (rss.max(1e-12) / n2).ln() + 2.0 * k + 2.0 * k * (k + 1.0) / denom
}

/// Fits affine and (with ≥ 3 tags) homography to the correspondences; returns the AICc
/// winner.
fn fit_models(corrs: &[TagCorr], cfg: &StageAConfig) -> Option<(Homography, FitModel)> {
    let src: Vec<PointPx> = corrs.iter().flat_map(|t| t.mm).collect();
    let dst: Vec<PointPx> = corrs.iter().flat_map(|t| t.px).collect();
    let affine = fit_affine(&src, &dst, None).map(|a| Homography::from_affine(&a));
    if corrs.len() < 3 {
        return affine.map(|h| (h, FitModel::Affine));
    }
    // Seeded RANSAC screens gross corner outliers; DLT on its inliers.
    let ransac = ransac_homography(
        &src,
        &dst,
        f64::from(cfg.ransac_threshold_mpx) / 1000.0,
        f64::from(cfg.ransac_confidence_permille) / 1000.0,
        cfg.ransac_max_iterations as usize,
        cfg.ransac_seed,
    );
    let homog = match ransac {
        Some(r) if r.inliers.iter().filter(|&&i| i).count() >= 8 => {
            let (si, di): (Vec<PointPx>, Vec<PointPx>) = src
                .iter()
                .zip(&dst)
                .zip(&r.inliers)
                .filter(|(_, ok)| **ok)
                .map(|((s, d), _)| (*s, *d))
                .unzip();
            fit_homography(&si, &di)
        }
        _ => fit_homography(&src, &dst),
    };
    let rss = |h: &Homography| residuals(h, corrs).iter().map(|r| r * r).sum::<f64>();
    match (affine, homog) {
        (Some(a), Some(h)) => {
            let n = src.len();
            if aicc(rss(&h), n, 8) < aicc(rss(&a), n, 6) {
                Some((h, FitModel::Homography))
            } else {
                Some((a, FitModel::Affine))
            }
        }
        (Some(a), None) => Some((a, FitModel::Affine)),
        (None, Some(h)) => Some((h, FitModel::Homography)),
        (None, None) => None,
    }
}

fn rms(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    (v.iter().map(|r| r * r).sum::<f64>() / v.len() as f64).sqrt()
}

/// Runs Stage A on the tags found by the corner search.
pub(crate) fn fit(
    img: &GrayImage,
    page: &PageGeometry,
    tags: &[TagHit],
    mirrored: bool,
    cfg: &StageAConfig,
    tcfg: &super::config::TagConfig,
) -> Result<StageA, StageAFailure> {
    let w = img.width() as f64;
    let search = f64::from(tcfg.edge_search_mpx) / 1000.0;
    let trim = f64::from(tcfg.edge_trim_permille) / 1000.0;
    let outlier = f64::from(tcfg.edge_outlier_mpx) / 1000.0;
    // Refine every tag on the original image (two passes: the second is seeded by the first,
    // so the result hardly depends on the detector's corners).
    let mut refined: Vec<(TagHit, Option<Refined>)> = tags
        .iter()
        .map(|t| {
            let r1 = refine_quad(img, &t.corners, search, trim, outlier);
            let r2 = r1.and_then(|r| refine_quad(img, &r.corners, search, trim, outlier));
            (*t, r2.or(r1))
        })
        .collect();
    let n_refined = refined.iter().filter(|(_, r)| r.is_some()).count();
    if n_refined >= 2 {
        refined.retain(|(_, r)| r.is_some());
    }
    if refined.len() < 2 {
        return Err(StageAFailure::TooFewTags);
    }
    let mut blacks: Vec<u8> = refined
        .iter()
        .filter_map(|(_, r)| r.map(|r| r.black))
        .collect();
    let mut whites: Vec<u8> = refined
        .iter()
        .filter_map(|(_, r)| r.map(|r| r.white))
        .collect();
    blacks.sort_unstable();
    whites.sort_unstable();
    let black = blacks.get(blacks.len() / 2).copied().unwrap_or(0);
    let white = whites.get(whites.len() / 2).copied().unwrap_or(255);

    // Corners in the hypothesis frame: mirrored pages are fitted in the mirrored image's
    // coordinates (x' = W − x), where the quads run clockwise again.
    let to_frame = |c: [PointPx; 4]| -> [PointPx; 4] {
        if mirrored {
            let mut m = c.map(|p| PointPx { x: w - p.x, y: p.y });
            m.reverse();
            m
        } else {
            c
        }
    };
    let quads: Vec<(u8, [PointPx; 4])> = refined
        .iter()
        .map(|(t, r)| {
            (
                t.corner() as u8,
                to_frame(r.map_or(t.corners, |r| r.corners)),
            )
        })
        .collect();
    let layout = |corner: u8| -> Option<[PointPx; 4]> {
        let tag = page.tags.get(usize::from(corner))?;
        Some(tag.corners_mm().map(|p| PointPx { x: p.x, y: p.y }))
    };
    // Initial similarity from the tag centres.
    let (cmm, cpx): (Vec<PointPx>, Vec<PointPx>) = quads
        .iter()
        .filter_map(|(c, q)| Some((centre(&layout(*c)?), centre(q))))
        .unzip();
    let sim = fit_similarity(&cmm, &cpx).ok_or(StageAFailure::Degenerate)?;
    let mut corrs: Vec<TagCorr> = Vec::new();
    for (c, q) in &quads {
        let Some(mm) = layout(*c) else { continue };
        let pred = mm.map(|p| sim.map(p));
        let best = (0..4)
            .min_by(|&a, &b| {
                let cost = |s: usize| -> f64 {
                    (0..4)
                        .map(|i| {
                            let d = q[(i + s) % 4];
                            (d.x - pred[i].x).powi(2) + (d.y - pred[i].y).powi(2)
                        })
                        .sum()
                };
                cost(a).total_cmp(&cost(b))
            })
            .unwrap_or(0);
        let px = [0, 1, 2, 3].map(|i| q[(i + best) % 4]);
        corrs.push(TagCorr { corner: *c, mm, px });
    }
    corrs.sort_by_key(|t| t.corner);
    let (mut h, mut model) = fit_models(&corrs, cfg).ok_or(StageAFailure::Degenerate)?;

    // Leave-one-out on tags (§8.4): drop a tag whose residual stands out, then refit.
    if corrs.len() >= 4 {
        let res = residuals(&h, &corrs);
        let per_tag: Vec<f64> = res.chunks(4).map(rms).collect();
        let mut sorted = per_tag.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let ppm = h.local_scale(PointPx { x: 105.0, y: 148.5 });
        let gate_px = f64::from(cfg.rms_warn_um) / 1000.0 * ppm;
        let limit = (f64::from(cfg.loo_factor_permille) / 1000.0 * median).max(gate_px);
        if let Some((worst, &r)) = per_tag
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
            && r > limit
        {
            corrs.remove(worst);
            (h, model) = fit_models(&corrs, cfg).ok_or(StageAFailure::Degenerate)?;
        }
    }
    let res = residuals(&h, &corrs);
    let rms_px = rms(&res);
    let max_px = res.iter().copied().fold(0.0, f64::max);

    // Diagnostics in the (un-mirrored) hypothesis frame at the page centre.
    let mid = PointPx { x: 105.0, y: 148.5 };
    let j = h.jacobian(mid);
    let theta = j[1][0].atan2(j[0][0]);
    let quarter = (theta / std::f64::consts::FRAC_PI_2).round();
    let skew = theta - quarter * std::f64::consts::FRAC_PI_2;
    let quarter_turns = (quarter as i64).rem_euclid(4) as u8;
    // Remove the quarter turns before decomposing.
    let (qs, qc) = (-quarter * std::f64::consts::FRAC_PI_2).sin_cos();
    let a = Affine::new(
        qc * j[0][0] - qs * j[1][0],
        qc * j[0][1] - qs * j[1][1],
        0.0,
        qs * j[0][0] + qc * j[1][0],
        qs * j[0][1] + qc * j[1][1],
        0.0,
    );
    let d = a.decompose();
    let px_per_mm = d.scale;

    // Back to original image coordinates.
    if mirrored {
        let f = [-1.0, 0.0, w, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        h = Homography { h: mul3(&f, &h.h) };
    }
    Ok(StageA {
        h,
        model,
        tags_used: corrs.iter().map(|t| t.corner).collect(),
        tags_refined: n_refined as u8,
        rms_px,
        rms_um: if px_per_mm > 0.0 {
            rms_px / px_per_mm * 1000.0
        } else {
            f64::INFINITY
        },
        max_px,
        mirrored,
        quarter_turns,
        skew_rad: skew,
        px_per_mm,
        anisotropy: d.anisotropy,
        black,
        white,
    })
}

/// How the capture resolution was established (§8.2 "Measured DPI").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpiSource {
    /// From the capture agent or device profile.
    Device(u32),
    /// From the file header (a standard value implying a plausible scale).
    Header(u32),
    /// Unknown: REG-004 `scale_unverifiable`.
    Unknown,
}

/// Picks the capture DPI (§8.2 rules 1, 2 and 4).
pub(crate) fn capture_dpi(
    device: Option<u32>,
    header: Option<u32>,
    dpi_eff: f64,
    cfg: &StageAConfig,
) -> DpiSource {
    if let Some(d) = device.filter(|&d| d > 0) {
        return DpiSource::Device(d);
    }
    if let Some(hd) = header.filter(|d| cfg.standard_dpis.contains(d)) {
        let s = dpi_eff / f64::from(hd) * 1000.0;
        if s >= f64::from(cfg.scale_warn_permille[0]) && s <= f64::from(cfg.scale_warn_permille[1])
        {
            return DpiSource::Header(hd);
        }
    }
    DpiSource::Unknown
}

/// Stage A gates (§8.4 table, §8.2): REG-002, REG-003 (residual; fewer than three tags),
/// REG-004, REG-005. Returns the exceptions and whether the page failed (an ERROR that stops
/// auto-acceptance).
pub(crate) fn gates(
    a: &StageA,
    dpi: DpiSource,
    cfg: &StageAConfig,
    page: Option<&str>,
) -> (Vec<Exception>, bool) {
    let mut out = Vec::new();
    let mut failed = false;
    let pe = |code: &str, sev: Severity| exc::on_page(exc::page(code, sev), page);
    if a.mirrored {
        out.push(
            pe("REG-005", Severity::Warn)
                .with_detail("tags decode only after mirroring; un-mirrored"),
        );
    }
    if a.quarter_turns != 0 {
        out.push(pe("REG-002", Severity::Info).with_detail(format!(
            "page rotated {}° clockwise in the image; auto-rotated",
            u32::from(a.quarter_turns) * 90
        )));
    }
    if a.tags_used.len() < 3 {
        // §8.4 tag-count rule: two tags need Stage B (forced) and Stage C. Without Stage B the
        // two-tag affine extrapolates across the page and cannot model perspective or feed
        // stretch (bubbles up to 15 px off on the standard slice, 30 px on the hard one; the
        // row ticks are then searched in the wrong place), so the page is never auto-graded.
        failed = true;
        out.push(pe("REG-003", Severity::Error).with_detail(format!(
            "only {} corner tags used: Stage A from {} corners extrapolates and Stage B is not \
             available; sheet review",
            a.tags_used.len(),
            a.tags_used.len() * 4
        )));
    }
    if a.rms_um > f64::from(cfg.rms_error_um) {
        failed = true;
        out.push(pe("REG-003", Severity::Error).with_detail(format!(
            "RMS anchor residual {:.0} µm > {} µm: sheet review",
            a.rms_um, cfg.rms_error_um
        )));
    } else if a.rms_um > f64::from(cfg.rms_warn_um) {
        out.push(pe("REG-003", Severity::Warn).with_detail(format!(
            "RMS anchor residual {:.0} µm > {} µm",
            a.rms_um, cfg.rms_warn_um
        )));
    }
    match dpi {
        DpiSource::Device(d) | DpiSource::Header(d) => {
            let s = (a.dpi_eff() / f64::from(d) * 1000.0).round() as u32;
            let [w0, w1] = cfg.scale_warn_permille;
            let [s0, s1] = cfg.scale_silent_permille;
            if s < w0 || s > w1 {
                failed = true;
                out.push(
                    pe("REG-004", Severity::Error)
                        .with_sub_reason("scale")
                        .with_detail(format!(
                            "print scale {:.3} outside 0.80–1.10 (wrong template or photocopy)",
                            f64::from(s) / 1000.0
                        )),
                );
            } else if s < s0 || s > s1 {
                out.push(
                    pe("REG-004", Severity::Warn)
                        .with_sub_reason("scale")
                        .with_detail(format!(
                            "print scale {:.3}: print at 100 %",
                            f64::from(s) / 1000.0
                        )),
                );
            }
        }
        DpiSource::Unknown => out.push(
            pe("REG-004", Severity::Warn)
                .with_sub_reason("scale_unverifiable")
                .with_detail(format!(
                    "capture DPI unknown; effective {:.1} dpi, per-bubble 20 px gate only",
                    a.dpi_eff()
                )),
        ),
    }
    if a.skew_rad.abs() * 1e6 > f64::from(cfg.skew_warn_urad) {
        out.push(
            pe("REG-004", Severity::Warn)
                .with_sub_reason("skew")
                .with_detail(format!("skew {:.2}° > 3°", a.skew_rad.to_degrees())),
        );
    }
    if a.anisotropy.abs() * 1000.0 > f64::from(cfg.anisotropy_warn_permille) {
        out.push(
            pe("REG-004", Severity::Warn)
                .with_sub_reason("anisotropy")
                .with_detail(format!("anisotropy {:.2} % > 2 %", a.anisotropy * 100.0)),
        );
    }
    (out, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_and_aicc() {
        let src = [
            PointPx { x: 0.0, y: 0.0 },
            PointPx { x: 10.0, y: 0.0 },
            PointPx { x: 0.0, y: 20.0 },
        ];
        let truth = Affine::similarity(7.9, 0.03, 100.0, -20.0);
        let dst: Vec<PointPx> = src.iter().map(|p| truth.map(*p)).collect();
        let s = fit_similarity(&src, &dst).unwrap();
        for (a, b) in s.m.iter().zip(truth.m) {
            assert!((a - b).abs() < 1e-9);
        }
        // More parameters must buy a real RSS reduction.
        assert!(aicc(1.0, 16, 8) > aicc(1.0, 16, 6));
        assert!(aicc(0.1, 16, 8) < aicc(1.0, 16, 6));
    }
}
