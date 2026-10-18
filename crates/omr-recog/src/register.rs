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
//! 6. **Shift-sensitive verification** (§8.4 "never silently wrong"): ICP can converge on a
//!    lattice alias — every bubble one row or column away from its true place, which still
//!    matches most rings. Each lattice region of the template (the answer blocks, the
//!    student-number, section and date grids, the version, exam and gender columns) is checked
//!    against its own one-pitch shifts on **non-periodic content**: the blank-form pixels that
//!    differ from their shifted selves (question-number boxes, titles and box borders, the
//!    empty bands beyond the first and last rows and columns), correlated with the rectified
//!    page at 1/4, plus **printed-ring support** (no shift may put more template bubbles on
//!    detected rings). The alignment must beat both shifts of every region and axis by a clear
//!    margin. A region that a one-pitch shift fits better is corrected (the warp is refitted on
//!    the shifted correspondences and ICP refines it), then verified again.
//!    - **Row and column checks** (adversarial verification): the region test passes on the part
//!      of a region that still matches, so a page whose lower part moved by whole rows against the
//!      rest (a fold, a torn and taped sheet, an ADF feed stall or slip) would be read rows off.
//!      Each answer row is therefore compared, at 1/2, with the question numbers of the rows up to
//!      six away (the blank form moved, not the page: a row hidden by a fold shows another row's
//!      number), and on scanner pages each answer column with the option letters of the columns up
//!      to four away. Another row's or column's content fitting clearly better (by ≥ 0.35, at
//!      ≥ 0.6) is positive evidence of a moved part: the alignment is not verified and nothing on
//!      the page is read. A row whose own number does not show at all is unreadable.
//! 7. **Camera path** (§8.10 adapted to forms without fiducials), tried when the scanner path
//!    fails or cannot be verified: the affine initialisation comes from the measured
//!    horizontal **and** vertical pitch vectors (phone photos are anisotropic and sheared), the
//!    coarse correlation then runs on that linear map, and ICP grows outwards from the template
//!    centre fitting homographies (a projective initialisation of the quadratic warp) before
//!    the quadratic rounds. It is verified exactly like the scanner path.
//!
//! Failure gates raise REG-001 (no lattice, no correlation), REG-003 (alias ambiguity, matched
//! count, RMS, an alignment that cannot be verified) and REG-004 (scale, skew, anisotropy);
//! REG-002 (INFO) logs an upside-down page. A failed page measures nothing and every bubble is
//! UNREADABLE: an alignment that no candidate verifies clearly is refused, never guessed.

use crate::config::RegistrationConfig;
use crate::exc;
use omr_imaging::{
    Affine, GrayImage, Homography, PatchTemplate, Peak, PointGrid, QuadWarp, RingHit, RingKernel,
    Warp, bilinear, fit_affine_lmeds, fit_quadratic, fit_quadratic_projective, local_maxima,
    match_patch, ncc_patch, ransac_homography, resample_affine_offset, ring_map_fast,
};
use omr_model::{Exception, PointPx, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How the accepted alignment was found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationMethod {
    /// Scanner path: pitch similarity, coarse correlation, ICP (the brief's algorithm).
    #[default]
    Lattice,
    /// Scanner path, then a verified one-pitch correction of at least one region.
    LatticeCorrected,
    /// Camera path: affine from both pitch vectors, projective growth, quadratic ICP.
    Camera,
    /// Camera path, then a verified one-pitch correction of at least one region.
    CameraCorrected,
}

impl RegistrationMethod {
    /// True for the camera path (phone geometry: perspective, anisotropic scale).
    pub fn is_camera(self) -> bool {
        matches!(
            self,
            RegistrationMethod::Camera | RegistrationMethod::CameraCorrected
        )
    }
}

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
    /// Shift-sensitive verification: the smallest margin by which the alignment's correlation
    /// with the blank form's non-periodic content beats a one-pitch shift, over every region
    /// and axis (negative: a shift fits better).
    #[serde(default)]
    pub verify_margin: f64,
    /// Region × axis tests in the verification.
    #[serde(default)]
    pub verify_tests: u32,
    /// How the alignment was found.
    #[serde(default)]
    pub method: RegistrationMethod,
    /// Gray capture read against its paper background (§8.5); bitonal pages keep `B = 255`.
    #[serde(default)]
    pub paper_normalised: bool,
    /// Coefficient of variation of the paper level under the bubbles (uneven light).
    #[serde(default)]
    pub illumination_cv: f64,
    /// Local verification: the smallest margin by which a band of rows beats every whole-row
    /// shift of itself (1.0 when no band was tested).
    #[serde(default)]
    pub window_margin: f64,
    /// Local verification: the smallest correlation of a band of rows with its own content
    /// (1.0 when no band was tested).
    #[serde(default)]
    pub window_floor: f64,
    /// Bands of rows that failed the local verification (their bubbles are UNREADABLE).
    #[serde(default)]
    pub windows_failed: u32,
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
    /// Per template bubble: its lattice region could not be verified because the page does
    /// not show it (cut-off scan, cropped photo); such bubbles are UNREADABLE unless already
    /// outside the image (REG-003 Region / CAP-009), never read on an unverified alignment.
    pub unverified: Vec<bool>,
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

/// The non-periodic content of one region along one shift axis.
#[derive(Clone, Debug)]
pub(crate) struct VerifySet {
    /// The one-pitch shift `k` (template px); the test compares shifts `0`, `+k`, `−k`.
    pub shift: PointPx,
    /// Template points (centres of 1/4-resolution pixels, template px).
    pub points: Vec<PointPx>,
    /// The blank form's 1/4-resolution value at each point.
    pub blank: Vec<u8>,
}

/// One lattice region of the template (a field, or one block of answers).
#[derive(Clone, Debug)]
pub(crate) struct VerifyRegion {
    /// Template bubble indices.
    pub members: Vec<usize>,
    /// Lattice pitch along x and y (template px), if the region has columns / rows.
    pub pitch: (Option<f64>, Option<f64>),
    /// One set per testable axis: (1, 0), (0, 1), (1, 1), (1, −1) in pitch units.
    pub sets: Vec<VerifySet>,
    /// Bands of rows tested on their own (local verification); empty for short regions.
    pub windows: Vec<VerifyWindow>,
}

/// A band of consecutive rows of a region (one row by default) and its own non-periodic
/// content at 1/2 (the blank-form pixels that differ from their one-row shift: the question
/// number, the digit or letter labels). The page under the band must look more like this
/// band's content than like the content of any band up to `verify_window_shift` rows above or
/// below, so a part of the page that moved against the rest by whole rows (a fold, a torn and
/// taped sheet, an ADF feed stall) is caught even when the region as a whole still verifies.
#[derive(Clone, Debug)]
pub(crate) struct VerifyWindow {
    /// Template bubble indices in the band.
    pub members: Vec<usize>,
    /// Template points (centres of 1/2-resolution pixels, template px).
    pub points: Vec<PointPx>,
    /// Per row shift `j = −S..=S`: the blank form at the points moved by `j` rows (`j = 0`:
    /// the band's own content).
    pub blank: Vec<Vec<u8>>,
    /// A row (its own content must show on the page); a column's own content lies inside its
    /// bubbles, under the students' marks, so only another column's content fitting better
    /// counts against it.
    pub row: bool,
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
    /// The verification regions.
    pub regions: Vec<VerifyRegion>,
    /// Region of each template bubble.
    pub region_of: Vec<usize>,
}

impl RefModel {
    /// `regions` partitions the template bubbles into lattice regions (indices into `points`);
    /// `row_labelled[r]`: region `r`'s rows are labelled outside their bubbles (question
    /// numbers), so each row is also checked on its own.
    pub(crate) fn new(
        points: Vec<PointPx>,
        radius: f64,
        blank: &GrayImage,
        regions: &[Vec<usize>],
        row_labelled: &[bool],
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
        let mut blank4 = None;
        let mut blank2 = None;
        while factor < 32 {
            img = omr_imaging::half(&img);
            factor *= 2;
            if factor == 2 {
                blank2 = Some(img.clone());
            }
            if factor == 4 {
                blank4 = Some(img.clone());
            }
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
        let blank4 = blank4.unwrap_or_else(|| GrayImage::new(1, 1, 255));
        let blank2 = blank2.unwrap_or_else(|| GrayImage::new(1, 1, 255));
        let mut region_of = vec![usize::MAX; points.len()];
        let regions: Vec<VerifyRegion> = regions
            .iter()
            .enumerate()
            .map(|(r, members)| {
                for &j in members {
                    if let Some(slot) = region_of.get_mut(j) {
                        *slot = r;
                    }
                }
                let pts: Vec<PointPx> = members
                    .iter()
                    .filter_map(|&j| points.get(j))
                    .copied()
                    .collect();
                let level = if cfg.verify_window_level == 2 {
                    &blank2
                } else {
                    &blank4
                };
                let mut v = verify_region(members.clone(), &pts, &blank4, level, radius, cfg);
                if !row_labelled.get(r).copied().unwrap_or(false) {
                    v.windows.clear();
                }
                v
            })
            .collect();
        Self {
            points,
            centre,
            radius,
            levels,
            kernels,
            regions,
            region_of,
        }
    }
}

/// Median nearest-neighbour distance along one axis (`along > 5 px`, `|across| < 0.3·along`).
fn lattice_pitch(pts: &[PointPx], along_x: bool) -> Option<f64> {
    let mut d = Vec::new();
    for p in pts {
        let mut best = f64::INFINITY;
        for q in pts {
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            let (along, across) = if along_x { (dx, dy) } else { (dy, dx) };
            if along > 5.0 && across.abs() < 0.3 * along && along < best {
                best = along;
            }
        }
        if best.is_finite() {
            d.push(best);
        }
    }
    median_f64(&mut d)
}

/// Builds a region's verification sets from the blank form at 1/4.
fn verify_region(
    members: Vec<usize>,
    pts: &[PointPx],
    blank4: &GrayImage,
    blank_w: &GrayImage,
    radius: f64,
    cfg: &RegistrationConfig,
) -> VerifyRegion {
    let pitch = (lattice_pitch(pts, true), lattice_pitch(pts, false));
    let mut sets = Vec::new();
    if pts.is_empty() {
        return VerifyRegion {
            members,
            pitch,
            sets,
            windows: Vec::new(),
        };
    }
    let (px, py) = (pitch.0.unwrap_or(0.0), pitch.1.unwrap_or(0.0));
    let (mx, my) = (1.5 * px + 20.0, 1.5 * py + 20.0);
    let lo_x = pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - mx;
    let hi_x = pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max) + mx;
    let lo_y = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - my;
    let hi_y = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max) + my;
    let (w4, h4) = (blank4.width() as i64, blank4.height() as i64);
    let i0 = ((lo_x / 4.0).floor() as i64).clamp(0, w4);
    let i1 = ((hi_x / 4.0).ceil() as i64).clamp(0, w4);
    let j0 = ((lo_y / 4.0).floor() as i64).clamp(0, h4);
    let j1 = ((hi_y / 4.0).ceil() as i64).clamp(0, h4);
    let tau = i32::try_from(cfg.verify_contrast).unwrap_or(i32::MAX);
    for (ax, ay) in [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, -1.0)] {
        if (ax != 0.0 && pitch.0.is_none()) || (ay != 0.0 && pitch.1.is_none()) {
            continue;
        }
        let k = PointPx {
            x: ax * px,
            y: ay * py,
        };
        let mut cand: Vec<(PointPx, u8)> = Vec::new();
        for j in j0..j1 {
            for i in i0..i1 {
                let t = PointPx {
                    x: 4.0 * i as f64 + 2.0,
                    y: 4.0 * j as f64 + 2.0,
                };
                let b0 = blank4.get(i as usize, j as usize);
                let bp = bilinear(blank4, (t.x - k.x) / 4.0, (t.y - k.y) / 4.0, 255);
                let bm = bilinear(blank4, (t.x + k.x) / 4.0, (t.y + k.y) / 4.0, 255);
                let d = (i32::from(b0) - i32::from(bp))
                    .abs()
                    .max((i32::from(b0) - i32::from(bm)).abs());
                if d >= tau {
                    cand.push((t, b0));
                }
            }
        }
        let cap = cfg.verify_points.max(2) as usize;
        if cand.len() > cap {
            let n = cand.len();
            cand = (0..cap)
                .map(|i| cand[(i * (n - 1) + (cap - 1) / 2) / (cap - 1)])
                .collect();
        }
        if cand.len() >= cfg.verify_min_points as usize {
            sets.push(VerifySet {
                shift: k,
                points: cand.iter().map(|c| c.0).collect(),
                blank: cand.iter().map(|c| c.1).collect(),
            });
        }
    }
    // The row checks reach further sideways than the region tests: the question-number boxes
    // left of an answer block (and the next block's, right of it) are what tells its rows apart.
    let reach = 2.5 * px + 40.0;
    let mut windows = match pitch.1 {
        Some(py) => verify_windows(
            &members,
            pts,
            py,
            (lo_x - reach, hi_x + reach),
            blank_w,
            radius,
            cfg,
        ),
        None => Vec::new(),
    };
    if let (Some(px), Some(py)) = pitch {
        windows.extend(verify_columns(&members, pts, px, py, blank_w, cfg));
    }
    VerifyRegion {
        members,
        pitch,
        sets,
        windows,
    }
}

/// The local-verification bands of a region with row pitch `py`: rows clustered by template y,
/// cut into bands of `verify_window_rows` rows, each with the blank-form pixels at 1/2 (template
/// x within `xs`: the region's width and its row labels) of its rows that differ from their
/// one-row shift by at least `verify_contrast`, and the blank form at those points moved by
/// every row shift up to `verify_window_shift`.
fn verify_windows(
    members: &[usize],
    pts: &[PointPx],
    py: f64,
    xs: (f64, f64),
    blank2: &GrayImage,
    radius: f64,
    cfg: &RegistrationConfig,
) -> Vec<VerifyWindow> {
    let f = if cfg.verify_window_level == 2 {
        2.0
    } else {
        4.0
    };
    let w = cfg.verify_window_rows as usize;
    if w == 0 || py <= 0.0 {
        return Vec::new();
    }
    let mut ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
    ys.sort_by(f64::total_cmp);
    let mut rows: Vec<f64> = Vec::new();
    for y in ys {
        if rows.last().is_none_or(|&r| y - r > 0.5 * py) {
            rows.push(y);
        }
    }
    if rows.len() < 3 {
        return Vec::new();
    }
    let bands: Vec<(f64, f64)> = rows
        .chunks(w)
        .map(|c| (c[0] - 0.5 * py, c[c.len() - 1] + 0.5 * py))
        .collect();
    let s_max = cfg.verify_window_shift as i32;
    let tau = i32::try_from(cfg.verify_contrast).unwrap_or(i32::MAX);
    let (w2, h2) = (blank2.width() as i64, blank2.height() as i64);
    let i0 = ((xs.0 / f).floor() as i64).clamp(0, w2);
    let i1 = ((xs.1 / f).ceil() as i64).clamp(0, w2);
    let cap = cfg.verify_window_points.max(2) as usize;
    let at = |x: f64, y: f64| bilinear(blank2, x / f, y / f, 255);
    // Marks change what is inside a bubble: rows are told apart by what is around them (the
    // question number), unless only the bubbles' own labels tell them apart (digit grids).
    let rr = (1.15 * radius) * (1.15 * radius);
    let in_bubble = |t: &PointPx| {
        pts.iter()
            .any(|p| (p.x - t.x).powi(2) + (p.y - t.y).powi(2) <= rr)
    };
    bands
        .into_iter()
        .filter_map(|(lo, hi)| {
            let j0 = ((lo / f).floor() as i64).clamp(0, h2);
            let j1 = ((hi / f).ceil() as i64).clamp(0, h2);
            let mut cand: Vec<PointPx> = Vec::new();
            let mut inside = 0usize;
            for j in j0..j1 {
                for i in i0..i1 {
                    let t = PointPx {
                        x: f * i as f64 + f / 2.0,
                        y: f * j as f64 + f / 2.0,
                    };
                    if t.y < lo || t.y >= hi {
                        continue;
                    }
                    let b0 = i32::from(blank2.get(i as usize, j as usize));
                    let bp = i32::from(at(t.x, t.y - py));
                    let bm = i32::from(at(t.x, t.y + py));
                    if (b0 - bp).abs().max((b0 - bm).abs()) >= tau {
                        if in_bubble(&t) {
                            inside += 1;
                        } else {
                            cand.push(t);
                        }
                    }
                }
            }
            // Rows told apart only by the labels inside their bubbles (the digit grids, where
            // marks and handwriting cover them) are left to the region test.
            let _ = inside;
            if cand.len() > cap {
                let n = cand.len();
                cand = (0..cap)
                    .map(|i| cand[(i * (n - 1) + (cap - 1) / 2) / (cap - 1)])
                    .collect();
            }
            if cand.len() < cfg.verify_window_min_points as usize {
                return None;
            }
            let blank = (-s_max..=s_max)
                .map(|k| {
                    cand.iter()
                        .map(|t| at(t.x, t.y + f64::from(k) * py))
                        .collect()
                })
                .collect();
            let band: Vec<usize> = members
                .iter()
                .zip(pts)
                .filter(|(_, p)| p.y >= lo && p.y < hi)
                .map(|(&j, _)| j)
                .collect();
            Some(VerifyWindow {
                members: band,
                points: cand,
                blank,
                row: true,
            })
        })
        .collect()
}

/// The column checks of a region with option pitch `px`: each column of bubbles with the
/// blank-form pixels of the column (its option letters, at the band level) that differ from
/// their one-column shift by at least `verify_contrast`, and the blank form at those points
/// moved by every column shift up to `verify_column_shift` — so a fold or a tear that moved part
/// of a block sideways by whole options is caught like a moved row.
fn verify_columns(
    members: &[usize],
    pts: &[PointPx],
    px: f64,
    py: f64,
    blank_w: &GrayImage,
    cfg: &RegistrationConfig,
) -> Vec<VerifyWindow> {
    let s_max = cfg.verify_column_shift as i32;
    if s_max == 0 || px <= 0.0 || pts.is_empty() {
        return Vec::new();
    }
    let f = if cfg.verify_window_level == 2 {
        2.0
    } else {
        4.0
    };
    let mut xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
    xs.sort_by(f64::total_cmp);
    let mut cols: Vec<f64> = Vec::new();
    for x in xs {
        if cols.last().is_none_or(|&c| x - c > 0.5 * px) {
            cols.push(x);
        }
    }
    if cols.len() < 3 {
        return Vec::new();
    }
    let lo_y = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - 0.5 * py;
    let hi_y = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max) + 0.5 * py;
    let tau = i32::try_from(cfg.verify_contrast).unwrap_or(i32::MAX);
    let (w2, h2) = (blank_w.width() as i64, blank_w.height() as i64);
    let j0 = ((lo_y / f).floor() as i64).clamp(0, h2);
    let j1 = ((hi_y / f).ceil() as i64).clamp(0, h2);
    let cap = cfg.verify_window_points.max(2) as usize;
    let at = |x: f64, y: f64| bilinear(blank_w, x / f, y / f, 255);
    cols.into_iter()
        .filter_map(|c| {
            let (lo, hi) = (c - 0.5 * px, c + 0.5 * px);
            let i0 = ((lo / f).floor() as i64).clamp(0, w2);
            let i1 = ((hi / f).ceil() as i64).clamp(0, w2);
            let mut cand: Vec<PointPx> = Vec::new();
            for j in j0..j1 {
                for i in i0..i1 {
                    let t = PointPx {
                        x: f * i as f64 + f / 2.0,
                        y: f * j as f64 + f / 2.0,
                    };
                    if t.x < lo || t.x >= hi {
                        continue;
                    }
                    let b0 = i32::from(blank_w.get(i as usize, j as usize));
                    let bp = i32::from(at(t.x - px, t.y));
                    let bm = i32::from(at(t.x + px, t.y));
                    if (b0 - bp).abs().max((b0 - bm).abs()) >= tau {
                        cand.push(t);
                    }
                }
            }
            if cand.len() > cap {
                let n = cand.len();
                cand = (0..cap)
                    .map(|i| cand[(i * (n - 1) + (cap - 1) / 2) / (cap - 1)])
                    .collect();
            }
            if cand.len() < cfg.verify_window_min_points as usize {
                return None;
            }
            let blank = (-s_max..=s_max)
                .map(|k| {
                    cand.iter()
                        .map(|t| at(t.x + f64::from(k) * px, t.y))
                        .collect()
                })
                .collect();
            let band: Vec<usize> = members
                .iter()
                .zip(pts)
                .filter(|(_, p)| p.x >= lo && p.x < hi)
                .map(|(&j, _)| j)
                .collect();
            Some(VerifyWindow {
                members: band,
                points: cand,
                blank,
                row: false,
            })
        })
        .collect()
}

/// The page pyramid: full (borrowed), 1/2, 1/4, 1/8, 1/16, 1/32.
pub(crate) struct Pyramid<'a> {
    page: &'a GrayImage,
    reduced: Vec<GrayImage>,
    /// The paper background at 1/4 when the levels from 1/4 down were flattened (§8.5).
    background: Option<GrayImage>,
}

impl<'a> Pyramid<'a> {
    pub(crate) fn new(page: &'a GrayImage) -> Self {
        let mut reduced: Vec<GrayImage> = Vec::with_capacity(5);
        for _ in 0..5 {
            let next = omr_imaging::half(reduced.last().unwrap_or(page));
            reduced.push(next);
        }
        Self {
            page,
            reduced,
            background: None,
        }
    }

    /// The pyramid of a gray capture: the 1/4 level is divided by its paper background (a
    /// closing of `radius_px` full-resolution px, then a blur), and the coarser levels are
    /// reduced from it, so the coarse correlation and the verification see even paper under
    /// shadows, vignetting and colour casts. Full and 1/2 resolution (ring detection, local
    /// NCC) stay as captured.
    pub(crate) fn flattened(page: &'a GrayImage, radius_px: u32) -> Self {
        let l2 = omr_imaging::half(page);
        let l4 = omr_imaging::half(&l2);
        let r4 = (radius_px as usize).div_ceil(4).max(1);
        let bg = omr_imaging::paper_background(&l4, r4);
        let mut reduced = Vec::with_capacity(5);
        reduced.push(l2);
        reduced.push(omr_imaging::flatten(&l4, &bg));
        for _ in 0..3 {
            let next = omr_imaging::half(reduced.last().expect("levels"));
            reduced.push(next);
        }
        Self {
            page,
            reduced,
            background: Some(bg),
        }
    }

    pub(crate) fn at(&self, factor: usize) -> &GrayImage {
        match factor {
            1 => self.page,
            f => &self.reduced[f.trailing_zeros() as usize - 1],
        }
    }

    /// The paper background at 1/4, if this pyramid was flattened.
    pub(crate) fn background(&self) -> Option<&GrayImage> {
        self.background.as_ref()
    }
}

fn mpx(v: u32) -> f64 {
    f64::from(v) / 1000.0
}

/// The option-pitch vector: the densest cluster of near-horizontal nearest-neighbour vectors.
pub(crate) fn pitch_vector(points: &[PointPx], cfg: &RegistrationConfig) -> Option<PointPx> {
    let (lo, hi) = (mpx(cfg.pitch_min_mpx), mpx(cfg.pitch_max_mpx));
    densest_vector(points, cfg, lo, hi, true, false)
}

/// The camera path's option-pitch vector: as [`pitch_vector`], but the mode is taken over a
/// ±2 px window of the vector histogram, so a lattice whose pitch varies across a photo
/// (perspective) still beats a small grid of constant pitch.
pub(crate) fn camera_pitch_vector(points: &[PointPx], cfg: &RegistrationConfig) -> Option<PointPx> {
    let (lo, hi) = (mpx(cfg.pitch_min_mpx), mpx(cfg.pitch_max_mpx));
    densest_vector(points, cfg, lo, hi, true, true)
}

/// The camera path's row-pitch vector: the densest (windowed) cluster of near-vertical
/// nearest-neighbour vectors — the answer rows, not the header grids' tighter rows.
pub(crate) fn row_pitch_vector(points: &[PointPx], cfg: &RegistrationConfig) -> Option<PointPx> {
    let (lo, hi) = (mpx(cfg.row_pitch_mpx[0]), mpx(cfg.row_pitch_mpx[1]));
    densest_vector(points, cfg, lo, hi, false, true)
}

fn densest_vector(
    points: &[PointPx],
    cfg: &RegistrationConfig,
    lo: f64,
    hi: f64,
    horizontal: bool,
    windowed: bool,
) -> Option<PointPx> {
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
            let (along, across) = if horizontal { (v.x, v.y) } else { (v.y, v.x) };
            if along > 0.0 && across.abs() < dyr * along && lo < along && along < hi {
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
        let c = if windowed {
            let mut sum = 0;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    sum += counts.get(&(k.0 + dx, k.1 + dy)).copied().unwrap_or(0);
                }
            }
            sum
        } else {
            c
        };
        if c > best {
            best = c;
            mode = *k;
        }
    }
    let (mx, my) = (mode.0 as f64, mode.1 as f64);
    let r = mpx(cfg.pitch_cluster_mpx) * if windowed { 1.5 } else { 1.0 };
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

/// The linear part searched by the coarse correlation.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// The scanner path: similarity from the option pitch (scale `s0`, angle `th0`).
    Similarity { s0: f64, th0: f64 },
    /// The camera path: template → page linear map from both pitch vectors.
    Linear { m: [f64; 4] },
}

impl Shape {
    /// The linear map for the orientation (`upside` adds 180°) and an optional scale
    /// multiplier (per-mille) and angle offset (µrad) hypothesis. The similarity arithmetic is
    /// exactly the brief's (`s0 · ds / 1000`, `th0 + da · 10⁻⁶`).
    fn lin(self, upside: bool, hyp: Option<(u32, i32)>) -> Affine {
        match self {
            Shape::Similarity { s0, th0 } => {
                let th = if upside {
                    th0 + std::f64::consts::PI
                } else {
                    th0
                };
                match hyp {
                    None => Affine::similarity(s0, th, 0.0, 0.0),
                    Some((ds, da)) => Affine::similarity(
                        s0 * f64::from(ds) / 1000.0,
                        th + f64::from(da) * 1e-6,
                        0.0,
                        0.0,
                    ),
                }
            }
            Shape::Linear { m } => {
                let sg = if upside { -1.0 } else { 1.0 };
                let base = Affine::new(sg * m[0], sg * m[1], 0.0, sg * m[2], sg * m[3], 0.0);
                match hyp {
                    None => base,
                    Some((ds, da)) => {
                        let h = Affine::similarity(
                            f64::from(ds) / 1000.0,
                            f64::from(da) * 1e-6,
                            0.0,
                            0.0,
                        );
                        // Template-frame hypothesis first, then the base map.
                        h.then(&base)
                    }
                }
            }
        }
    }
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

/// Coarse alignment (non-periodic): the page, un-mapped by the shape's linear part, is
/// correlated with the central crop of the blank form: a full translation search at 1/32,
/// refinement at 1/16, the scale × angle hypotheses at 1/16, the five best at 1/8, the best
/// at 1/4 — then an explicit check that no translation by one lattice pitch correlates better.
fn coarse(pyr: &Pyramid, rm: &RefModel, shape: Shape, cfg: &RegistrationConfig) -> Option<Coarse> {
    let flat = f64::from(cfg.flat_variance_ppm) * 1e-6;
    let level = |f: usize| rm.levels.iter().find(|l| l.factor == f);
    let (l32, l16, l8, l4) = (level(32)?, level(16)?, level(8)?, level(4)?);
    // The page resampled into the reference frame over the shifts
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
    // 1/32: full translation search at the pitch hypothesis, upright and upside down (the
    // lattice pitch cannot tell a page fed the other way round, §8.3, REG-002).
    let p32 = pyr.at(32);
    let pad = (f64::from(cfg.coarse_pad_px) / 32.0).ceil() as isize;
    let (cw, ch) = (l32.crop.width() as isize, l32.crop.height() as isize);
    let full = |upside: bool| {
        let m = shape.lin(upside, None);
        // Page extent in the un-mapped frame (corners mapped back).
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
    let upright = full(false);
    let flipped = full(true);
    let orientation_margin = match (upright, flipped) {
        (Some(u), Some(f)) => (u.2 - f.2).abs(),
        _ => 1.0,
    };
    let ((e32x, e32y, _), upside_down) = match (upright, flipped) {
        (Some(u), Some(f)) if f.2 > u.2 => (f, true),
        (Some(u), _) => (u, false),
        (None, Some(f)) => (f, true),
        (None, None) => return None,
    };
    let m0 = shape.lin(upside_down, None);
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
    let mut hyps: Vec<(f64, (u32, i32), isize, isize)> = Vec::new();
    for &ds in &cfg.coarse_scale_permille {
        for &da in &cfg.coarse_angle_urad {
            let m = shape.lin(upside_down, Some((ds, da)));
            let u = m.inverse()?.map(anchor);
            let ex = ((u.x - rc.x) / 16.0).round() as isize;
            let ey = ((u.y - rc.y) / 16.0).round() as isize;
            if let Some((x, y, v)) = search(l16, &m, ex - 2, ey - 2, 5, 5) {
                hyps.push((v, (ds, da), x, y));
            }
        }
    }
    hyps.sort_by(|a, b| b.0.total_cmp(&a.0));
    // 1/8: the five best.
    let mut best8: Option<(f64, (u32, i32), isize, isize)> = None;
    for &(_, h, x, y) in hyps.iter().take(5) {
        let m = shape.lin(upside_down, Some(h));
        if let Some((ex, ey, v)) = search(l8, &m, 2 * x - 2, 2 * y - 2, 5, 5)
            && best8.is_none_or(|b| v > b.0)
        {
            best8 = Some((v, h, ex, ey));
        }
    }
    let (_, h, x8, y8) = best8?;
    let m = shape.lin(upside_down, Some(h));
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

/// The detected bubble lattice of a page (steps 1–3).
struct Lattice {
    rings: Vec<RingHit>,
    pts: Vec<PointPx>,
    grid: PointGrid,
    /// Option-pitch vector (page px).
    v: PointPx,
    /// Half-resolution ring candidates (full-resolution px).
    cands: Vec<PointPx>,
    /// Detection radius used.
    radius: f64,
}

/// Steps 1–3: candidates, pitch, full-resolution rings.
fn lattice(
    page: &GrayImage,
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
    q: &mut RegistrationQuality,
) -> Result<Lattice, Box<(Vec<RingHit>, Exception)>> {
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
        return Err(Box::new((
            Vec::new(),
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                "no bubble lattice found (no option pitch)".into(),
            ),
        )));
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
        return Err(Box::new((
            Vec::new(),
            reg_exc("REG-001", Severity::Blocker, None, "no ring kernels".into()),
        )));
    };
    q.ring_radius_px = kernel.radius();
    // 3. Full-resolution rings.
    let rings = detect_rings(page, &cands, kernel, cfg);
    q.detections = rings.len() as u32;
    if rings.len() < cfg.min_rings as usize {
        let detail = format!("only {} bubble rings detected", q.detections);
        return Err(Box::new((
            rings,
            reg_exc("REG-001", Severity::Blocker, None, detail),
        )));
    }
    let pts: Vec<PointPx> = rings.iter().map(|r| r.centre).collect();
    let v = pitch_vector(&pts, cfg).unwrap_or(v0);
    q.pitch_px = (v.x * v.x + v.y * v.y).sqrt();
    let grid = PointGrid::new(&pts, 16.0);
    Ok(Lattice {
        rings,
        pts,
        grid,
        v,
        cands,
        radius: kernel.radius(),
    })
}

/// The camera path's lattice: phone photos are anisotropic, perspective changes the scale
/// across the page and blur softens the rings, so the scanner path's single candidate radius
/// and pitch-derived kernel can miss most rings (they are ellipses of varying size). The
/// candidate radii of the configuration are all tried, each with the kernels within 1.5 px of
/// it, and the ring set with the most detections is kept; the option and row pitch vectors
/// then come from those rings (windowed modes).
fn camera_lattice(
    page: &GrayImage,
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
) -> Option<(Lattice, PointPx)> {
    let mut best: (Vec<RingHit>, f64, Vec<PointPx>) =
        (lat.rings.clone(), lat.radius, lat.cands.clone());
    let radii = std::iter::once(cfg.candidate_radius_mpx)
        .chain(cfg.fallback_candidate_radii_mpx.iter().copied());
    for (i, cr) in radii.enumerate() {
        let cands = if i == 0 {
            lat.cands.clone()
        } else {
            candidates(pyr.at(2), mpx(cr), cfg)
        };
        for k in rm
            .kernels
            .iter()
            .filter(|k| (k.radius() - mpx(cr)).abs() <= 1.5 + 1e-9)
        {
            if i == 0 && (k.radius() - lat.radius).abs() < 1e-9 {
                continue;
            }
            let rings = detect_rings(page, &cands, k, cfg);
            if rings.len() > best.0.len() {
                best = (rings, k.radius(), cands.clone());
            }
        }
    }
    let (rings, radius, cands) = best;
    if rings.len() < cfg.min_rings as usize {
        return None;
    }
    let pts: Vec<PointPx> = rings.iter().map(|r| r.centre).collect();
    let v = camera_pitch_vector(&pts, cfg).unwrap_or(lat.v);
    let vr = row_pitch_vector(&pts, cfg).or_else(|| row_pitch_vector(&cands, cfg))?;
    let grid = PointGrid::new(&pts, 16.0);
    Some((
        Lattice {
            rings,
            pts,
            grid,
            v,
            cands,
            radius,
        },
        vr,
    ))
}

/// The scanner ICP (step 5): LMedS affine rounds, then weighted quadratic rounds.
fn icp_scanner(rm: &RefModel, cfg: &RegistrationConfig, lat: &Lattice, a0: &Affine) -> QuadWarp {
    let n_t = rm.points.len();
    let norm = f64::from(cfg.quad_norm_px);
    let mut warp = QuadWarp::from_affine(a0, rm.centre, norm);
    let huber = mpx(cfg.huber_mpx);
    let mut src = Vec::with_capacity(n_t);
    let mut dst = Vec::with_capacity(n_t);
    let mut wts = Vec::with_capacity(n_t);
    for (it, &thr) in cfg.icp_thresholds_mpx.iter().enumerate() {
        src.clear();
        dst.clear();
        wts.clear();
        for t in &rm.points {
            if let Some((k, d)) = lat.grid.nearest_within(warp.map(*t), mpx(thr)) {
                src.push(*t);
                dst.push(lat.pts[k]);
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
    warp
}

/// Weighted quadratic ICP rounds from a prediction (after a correction, and the camera
/// path's last rounds); `den` is the fixed projective denominator of the fitted warp (zero:
/// the plain quadratic).
fn icp_quadratic(
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
    predict: &dyn Fn(PointPx) -> PointPx,
    den: [f64; 2],
) -> Option<QuadWarp> {
    let norm = f64::from(cfg.quad_norm_px);
    let huber = mpx(cfg.huber_mpx);
    let mut warp: Option<QuadWarp> = None;
    for &thr in &cfg.refine_icp_mpx {
        let mut src = Vec::new();
        let mut dst = Vec::new();
        let mut wts = Vec::new();
        for t in &rm.points {
            let p = match &warp {
                Some(w) => w.map(*t),
                None => predict(*t),
            };
            if let Some((k, d)) = lat.grid.nearest_within(p, mpx(thr)) {
                src.push(*t);
                dst.push(lat.pts[k]);
                wts.push(1.0 / (d / huber).max(1.0));
            }
        }
        if src.len() < cfg.icp_min_matches as usize {
            return warp;
        }
        if let Some(w) = fit_quadratic_projective(&src, &dst, Some(&wts), rm.centre, norm, den) {
            warp = Some(w);
        }
    }
    warp
}

/// The camera ICP: from the coarse affine, grow outwards from the template centre fitting a
/// robust affine, then homographies (a projective initialisation for photos), then the
/// quadratic rounds.
fn icp_camera(
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
    a0: &Affine,
) -> Option<QuadWarp> {
    let mut h = Homography::from_affine(a0);
    let mut fitted_affine = false;
    let mut step = 0u64;
    for &radius in &cfg.camera_radii_px {
        let r2 = f64::from(radius) * f64::from(radius);
        for &thr in &cfg.camera_icp_mpx {
            step += 1;
            let mut src = Vec::new();
            let mut dst = Vec::new();
            for t in &rm.points {
                let (dx, dy) = (t.x - rm.centre.x, t.y - rm.centre.y);
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                if let Some((k, _)) = lat.grid.nearest_within(h.map(*t), mpx(thr)) {
                    src.push(*t);
                    dst.push(lat.pts[k]);
                }
            }
            if src.len() < 12 {
                continue;
            }
            if !fitted_affine {
                if let Some(f) = fit_affine_lmeds(
                    &src,
                    &dst,
                    cfg.lmeds_iterations as usize,
                    cfg.lmeds_seed.wrapping_add(100 + step),
                ) {
                    h = Homography::from_affine(&f.model);
                    fitted_affine = true;
                }
            } else if let Some(f) = ransac_homography(
                &src,
                &dst,
                mpx(cfg.camera_ransac_mpx),
                0.999,
                500,
                cfg.lmeds_seed.wrapping_add(200 + step),
            ) {
                h = f.model;
            }
        }
    }
    if !fitted_affine {
        return None;
    }
    // The quadratic rounds keep the homography's projective denominator: a rational warp
    // (perspective exactly, plus a quadratic residual for lens and paper).
    let den = QuadWarp::from_homography(&h, rm.centre, f64::from(cfg.quad_norm_px))
        .map_or([0.0; 2], |q| q.den);
    icp_quadratic(rm, cfg, lat, &|t| h.map(t), den)
}

/// Final matched count, RMS, decomposition and the page gates (REG-003/004).
fn assess(
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
    warp: &QuadWarp,
    upside_down: bool,
    q: &mut RegistrationQuality,
) -> Result<(Vec<bool>, Vec<Exception>), Exception> {
    let n_t = rm.points.len();
    let mut matched = vec![false; n_t];
    let mut ss = 0.0;
    let mut nm = 0u32;
    for (j, t) in rm.points.iter().enumerate() {
        if let Some((_, d)) = lat
            .grid
            .nearest_within(warp.map(*t), mpx(cfg.final_match_mpx))
        {
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
        return Err(reg_exc(
            "REG-003",
            Severity::Error,
            None,
            format!(
                "registration failed: {} of {} bubbles matched, RMS {:.2} px",
                nm, n_t, q.rms_px
            ),
        ));
    }
    let sc = q.scale * 1000.0;
    if sc < f64::from(cfg.scale_warn_permille[0]) || sc > f64::from(cfg.scale_warn_permille[1]) {
        return Err(reg_exc(
            "REG-004",
            Severity::Error,
            Some("scale"),
            format!(
                "print scale {:.3} outside 0.80–1.10 (wrong template or photocopy)",
                q.scale
            ),
        ));
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
    Ok((matched, exceptions))
}

/// The quadratic warp of the verification, evaluated with a precomputed reciprocal (the same
/// polynomial as `QuadWarp::map`; deterministic, faster for the many samples of a check).
struct FastQuad {
    centre: PointPx,
    inv: f64,
    cx: [f64; 6],
    cy: [f64; 6],
    den: Option<[f64; 2]>,
}

impl FastQuad {
    fn new(w: &QuadWarp) -> Self {
        Self {
            centre: w.centre,
            inv: 1.0 / w.norm,
            cx: w.cx,
            cy: w.cy,
            den: w.is_rational().then_some(w.den),
        }
    }

    #[inline]
    fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let u = (x - self.centre.x) * self.inv;
        let v = (y - self.centre.y) * self.inv;
        let (uu, uv, vv) = (u * u, u * v, v * v);
        let (a, b) = (&self.cx, &self.cy);
        let x = a[0] + a[1] * u + a[2] * v + a[3] * uu + a[4] * uv + a[5] * vv;
        let y = b[0] + b[1] * u + b[2] * v + b[3] * uu + b[4] * uv + b[5] * vv;
        match self.den {
            Some(d) => {
                let w = 1.0 + d[0] * u + d[1] * v;
                (x / w, y / w)
            }
            None => (x, y),
        }
    }
}

/// Bilinear sample with 8-bit weights at a point (pixel `i` centred at `i + 0.5`), 255 outside;
/// floors by an offset cast instead of a libm call (same arithmetic on every platform).
#[inline]
fn sample(img: &GrayImage, x: f64, y: f64) -> i64 {
    const OFF: f64 = 1_073_741_824.0;
    let fx = x * 256.0 - 127.5;
    let fy = y * 256.0 - 127.5;
    if !(fx > -1.0e8 && fx < 1.0e8 && fy > -1.0e8 && fy < 1.0e8) {
        return 255;
    }
    let qx = (fx + OFF) as i64 - OFF as i64;
    let qy = (fy + OFF) as i64 - OFF as i64;
    let (ix, iy) = (qx >> 8, qy >> 8);
    let (wx, wy) = (qx & 255, qy & 255);
    let (w, h) = (img.width() as i64, img.height() as i64);
    let px = |xx: i64, yy: i64| -> i64 {
        if xx >= 0 && yy >= 0 && xx < w && yy < h {
            i64::from(img.as_raw()[(yy * w + xx) as usize])
        } else {
            255
        }
    };
    let top = px(ix, iy) * (256 - wx) + px(ix + 1, iy) * wx;
    let bot = px(ix, iy + 1) * (256 - wx) + px(ix + 1, iy + 1) * wx;
    (top * (256 - wy) + bot * wy + 32_768) >> 16
}

/// Correlations of one axis test at its three shifts, over the points whose three samples
/// all fall on the page (a cut-off scan or a cropped photo tests what it shows); `None` when
/// fewer than `min` points remain.
fn axis_correlations(
    set: &VerifySet,
    page4: &GrayImage,
    warp: &FastQuad,
    shifts: &[PointPx; 3],
    min: usize,
) -> Option<[f64; 3]> {
    let (w4, h4) = (page4.width() as f64, page4.height() as f64);
    let mut sums = [[0i64; 3]; 3]; // per shift: Σp, Σp², Σbp
    let (mut sb, mut sbb, mut n) = (0i64, 0i64, 0i64);
    let mut px = [0i64; 3];
    'points: for (t, &b) in set.points.iter().zip(&set.blank) {
        for (k, c) in shifts.iter().enumerate() {
            let (qx, qy) = warp.map(t.x - c.x, t.y - c.y);
            let (x, y) = (qx * 0.25, qy * 0.25);
            if !(x >= 0.0 && y >= 0.0 && x <= w4 && y <= h4) {
                continue 'points;
            }
            px[k] = sample(page4, x, y);
        }
        let b = i64::from(b);
        sb += b;
        sbb += b * b;
        n += 1;
        for k in 0..3 {
            sums[k][0] += px[k];
            sums[k][1] += px[k] * px[k];
            sums[k][2] += b * px[k];
        }
    }
    if (n as usize) < min.max(2) {
        return None;
    }
    let vb = (n * sbb - sb * sb) as f64;
    Some(sums.map(|[sp, spp, sbp]| {
        let vp = (n * spp - sp * sp) as f64;
        if vb <= 0.0 || vp <= 0.0 {
            -1.0
        } else {
            (n * sbp - sb * sp) as f64 / (vb * vp).sqrt()
        }
    }))
}

/// Correlations of the page under one band (sampled at `W(t)` on the 1/2 level) with the blank
/// form's content of that band and of the bands up to `S` rows away, over the points on the
/// page; `None` when fewer than `min` points remain.
fn band_correlations(
    w: &VerifyWindow,
    page2: &GrayImage,
    scale: f64,
    background: Option<&GrayImage>,
    warp: &FastQuad,
    min: usize,
) -> Option<Vec<f64>> {
    let (w2, h2) = (page2.width() as f64, page2.height() as f64);
    let mut obs = Vec::with_capacity(w.points.len());
    let mut keep = Vec::with_capacity(w.points.len());
    for (k, t) in w.points.iter().enumerate() {
        let (qx, qy) = warp.map(t.x, t.y);
        let (x, y) = (qx * scale, qy * scale);
        if x >= 0.0 && y >= 0.0 && x <= w2 && y <= h2 {
            let v = sample(page2, x, y);
            // A gray capture is read against its paper background (shadows and vignetting
            // across a row would otherwise dominate its correlations); bitonal pages as they are.
            obs.push(match background {
                Some(bg) => (v * 255 / sample(bg, qx * 0.25, qy * 0.25).max(1)).min(255),
                None => v,
            });
            keep.push(k);
        }
    }
    let n = obs.len() as i64;
    if (n as usize) < min.max(2) {
        return None;
    }
    let sp: i64 = obs.iter().sum();
    let spp: i64 = obs.iter().map(|p| p * p).sum();
    let vp = (n * spp - sp * sp) as f64;
    Some(
        w.blank
            .iter()
            .map(|b| {
                let (mut sb, mut sbb, mut sbp) = (0i64, 0i64, 0i64);
                for (&k, &p) in keep.iter().zip(&obs) {
                    let b = i64::from(b[k]);
                    sb += b;
                    sbb += b * b;
                    sbp += b * p;
                }
                let vb = (n * sbb - sb * sb) as f64;
                if vb <= 0.0 || vp <= 0.0 {
                    -1.0
                } else {
                    (n * sbp - sb * sp) as f64 / (vb * vp).sqrt()
                }
            })
            .collect(),
    )
}

/// Pearson correlation of the blank values with the page sampled at `W(t − c)` (1/4 level).
fn shift_correlation(set: &VerifySet, page4: &GrayImage, warp: &FastQuad, c: PointPx) -> f64 {
    let (mut sb, mut sp, mut sbb, mut spp, mut sbp) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for (t, &b) in set.points.iter().zip(&set.blank) {
        let (qx, qy) = warp.map(t.x - c.x, t.y - c.y);
        let p = sample(page4, qx * 0.25, qy * 0.25);
        let b = i64::from(b);
        sb += b;
        sp += p;
        sbb += b * b;
        spp += p * p;
        sbp += b * p;
    }
    let n = set.points.len() as i64;
    let vb = (n * sbb - sb * sb) as f64;
    let vp = (n * spp - sp * sp) as f64;
    if n < 2 || vb <= 0.0 || vp <= 0.0 {
        return -1.0;
    }
    (n * sbp - sb * sp) as f64 / (vb * vp).sqrt()
}

/// Template bubbles of a region with a detected ring within the snap radius of `W(t − c)`.
fn ring_support(
    rm: &RefModel,
    lat: &Lattice,
    cfg: &RegistrationConfig,
    region: &VerifyRegion,
    warp: &FastQuad,
    c: PointPx,
) -> u32 {
    let snap = mpx(cfg.snap_mpx);
    region
        .members
        .iter()
        .filter(|&&j| {
            let t = rm.points[j];
            let (x, y) = warp.map(t.x - c.x, t.y - c.y);
            lat.grid.nearest_within(PointPx { x, y }, snap).is_some()
        })
        .count() as u32
}

/// One region × axis test: correlations and ring support at shifts `0`, `+k`, `−k`.
#[derive(Clone, Debug)]
struct AxisTest {
    region: usize,
    shift: PointPx,
    ncc: [f64; 3],
    rings: [u32; 3],
}

impl AxisTest {
    fn margin(&self) -> f64 {
        self.ncc[0] - self.ncc[1].max(self.ncc[2])
    }

    fn passed(&self, cfg: &RegistrationConfig) -> bool {
        self.ncc[0] >= f64::from(cfg.verify_floor_milli) / 1000.0
            && self.margin() >= f64::from(cfg.verify_margin_milli) / 1000.0
            && self.rings[0] + cfg.verify_ring_tolerance >= self.rings[1].max(self.rings[2])
    }
}

/// The shift-sensitive verification of one alignment.
struct Verification {
    tests: Vec<AxisTest>,
    /// Regions the page does not show (no axis had enough points on the page).
    untested: Vec<usize>,
    /// Local verification: per tested band, (region, band, how much better the best other
    /// row's content fits the page than the band's own, the band's own correlation, a row).
    bands: Vec<(usize, usize, f64, f64, bool)>,
    /// A gray capture (read against its paper background): noisier band correlations.
    gray: bool,
}

impl Verification {
    /// Every tested axis passes, and enough of the page was tested to mean something.
    fn passed(&self, cfg: &RegistrationConfig) -> bool {
        self.tests.len() >= MIN_VERIFY_TESTS && self.tests.iter().all(|t| t.passed(cfg))
    }

    fn margin(&self) -> f64 {
        self.tests
            .iter()
            .map(AxisTest::margin)
            .fold(f64::INFINITY, f64::min)
    }

    /// The smallest band margin: own content minus the best other row's (1.0 when no band was
    /// tested; negative: another row's content fits better).
    fn band_margin(&self) -> f64 {
        self.bands.iter().map(|b| -b.2).fold(1.0, f64::min)
    }

    /// Bands where another row's content fits the page clearly better than the band's own
    /// (positive evidence that part of the page moved by whole rows): (region, band).
    fn moved_bands(&self, cfg: &RegistrationConfig) -> Vec<(usize, usize)> {
        let m = f64::from(cfg.verify_window_margin_milli) / 1000.0;
        let e = f64::from(cfg.verify_window_evidence_milli) / 1000.0;
        self.bands
            .iter()
            .filter(|b| b.2 >= m && b.3 + b.2 >= e)
            .map(|b| (b.0, b.1))
            .collect()
    }

    /// Bands whose own content does not correlate with the page at all (hidden or blank
    /// paper): (region, band).
    fn missing_bands(&self, cfg: &RegistrationConfig) -> Vec<(usize, usize)> {
        let floor = f64::from(if self.gray {
            cfg.verify_window_gray_floor_milli
        } else {
            cfg.verify_window_floor_milli
        }) / 1000.0;
        self.bands
            .iter()
            .filter(|b| b.4 && b.3 <= floor)
            .map(|b| (b.0, b.1))
            .collect()
    }

    /// The smallest own correlation of a row band (1.0 when no row was tested).
    fn band_floor(&self) -> f64 {
        self.bands
            .iter()
            .filter(|b| b.4)
            .map(|b| b.3)
            .fold(1.0, f64::min)
    }

    fn describe(&self, cfg: &RegistrationConfig) -> String {
        let failed: Vec<String> = self
            .tests
            .iter()
            .filter(|t| !t.passed(cfg))
            .map(|t| {
                format!(
                    "region {} shift ({:.0}, {:.0}) px: NCC {:.2} vs {:.2}/{:.2}, rings {} vs {}/{}",
                    t.region,
                    t.shift.x,
                    t.shift.y,
                    t.ncc[0],
                    t.ncc[1],
                    t.ncc[2],
                    t.rings[0],
                    t.rings[1],
                    t.rings[2]
                )
            })
            .collect();
        failed.join("; ")
    }
}

fn verify(
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
    warp: &QuadWarp,
) -> Verification {
    let page4 = pyr.at(4);
    let warp = &FastQuad::new(warp);
    let mut tests = Vec::new();
    let mut untested = Vec::new();
    for (r, region) in rm.regions.iter().enumerate() {
        if region.sets.is_empty() {
            continue;
        }
        let mut at0 = None;
        let mut tested = false;
        for set in &region.sets {
            let k = set.shift;
            let shifts = [PointPx { x: 0.0, y: 0.0 }, k, PointPx { x: -k.x, y: -k.y }];
            let Some(ncc) =
                axis_correlations(set, page4, warp, &shifts, cfg.verify_min_points as usize)
            else {
                continue;
            };
            tested = true;
            let r0 = *at0.get_or_insert_with(|| {
                ring_support(rm, lat, cfg, region, warp, PointPx::default())
            });
            tests.push(AxisTest {
                region: r,
                shift: k,
                ncc,
                rings: [
                    r0,
                    ring_support(rm, lat, cfg, region, warp, shifts[1]),
                    ring_support(rm, lat, cfg, region, warp, shifts[2]),
                ],
            });
        }
        if !tested {
            untested.push(r);
        }
    }
    // Local verification: the page under each band must look like that band, not like a
    // band up to S rows away.
    let mut bands = Vec::new();
    let lvl = if cfg.verify_window_level == 2 { 2 } else { 4 };
    let page2 = pyr.at(lvl);
    let scale = 1.0 / lvl as f64;
    // The columns are told apart only by the option letters inside their bubbles: readable on
    // a scanner page, not reliably on a photo (blur, perspective), so photos check rows only.
    let columns = pyr.background().is_none();
    for (r, region) in rm.regions.iter().enumerate() {
        for (b, w) in region.windows.iter().enumerate() {
            if !w.row && !columns {
                continue;
            }
            let Some(ncc) = band_correlations(
                w,
                page2,
                scale,
                pyr.background(),
                warp,
                cfg.verify_window_min_points as usize,
            ) else {
                continue;
            };
            let mid = ncc.len() / 2;
            let at0 = ncc[mid];
            let best_other = ncc
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != mid)
                .map(|(_, v)| *v)
                .fold(f64::NEG_INFINITY, f64::max);
            bands.push((r, b, best_other - at0, at0, w.row));
        }
    }
    Verification {
        tests,
        untested,
        bands,
        gray: !columns,
    }
}

/// Positive evidence that part of the page moved against the rest by whole rows: bands of
/// answer rows whose page content fits another row's question number clearly better than their
/// own (a fold, a torn and taped sheet, a scanner feed stall or slip), so the region-wide test
/// passed on the part that still matches. The alignment is then not verified and nothing on the
/// page is read (rows next to the damage may show no content at all): the detail for REG-003.
fn moved_detail(cfg: &RegistrationConfig, v: &Verification) -> Option<String> {
    let moved = v.moved_bands(cfg);

    (!moved.is_empty()).then(|| {
        format!(
            "part of the page moved against the rest by whole bubble rows ({} row(s) show \
             another row's question number: a fold, a torn and taped sheet, or a scanner feed \
             stall); rescan the sheet flat (row margin {:.2})",
            moved.len(),
            v.band_margin()
        )
    })
}

/// The local verification of a verified alignment: a band whose own content is missing from
/// the page (no correlation: covered, cut or folded away) leaves its bubbles UNREADABLE
/// (REG-003 Region, with a REG-003 WARN naming the rows). Returns the bubbles not read.
fn local_check(
    rm: &RefModel,
    cfg: &RegistrationConfig,
    v: &Verification,
    q: &mut RegistrationQuality,
    exceptions: &mut Vec<Exception>,
) -> Vec<usize> {
    q.window_margin = v.band_margin();
    q.window_floor = v.band_floor();
    let missing = v.missing_bands(cfg);
    q.windows_failed = missing.len() as u32;
    if missing.is_empty() {
        return Vec::new();
    }
    let mut local: Vec<usize> = missing
        .iter()
        .filter_map(|&(r, b)| rm.regions.get(r)?.windows.get(b))
        .flat_map(|w| w.members.iter().copied())
        .collect();
    local.sort_unstable();
    local.dedup();
    exceptions.push(reg_exc(
        "REG-003",
        Severity::Warn,
        None,
        format!(
            "{} bubble row(s) do not show their question number where the page should (covered, \
             cut or folded away): their {} bubbles are unreadable and sent to review",
            missing.len(),
            local.len()
        ),
    ));
    local
}

/// A verification needs at least this many region × axis tests (a page showing almost none of
/// the form verifies nothing).
const MIN_VERIFY_TESTS: usize = 4;

/// What the non-periodic content says about one region's alignment.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RegionCall {
    /// No shift: the alignment beats every one-pitch shift by the margin.
    Stay,
    /// The region sits one shift `c` away: `W(t − c)` beats the alignment by the margin.
    Shift(PointPx),
    /// Neither: left out of the refit.
    Unknown,
}

/// Per region: the one-pitch shift (rows and columns, up to two rows) that correlates best
/// with the region's non-periodic content, judged against no shift by the verification margin.
fn region_call(
    pyr: &Pyramid,
    cfg: &RegistrationConfig,
    region: &VerifyRegion,
    warp: &QuadWarp,
) -> RegionCall {
    let page4 = pyr.at(4);
    let warp = &FastQuad::new(warp);
    let union = VerifySet {
        shift: PointPx::default(),
        points: region.sets.iter().flat_map(|s| s.points.clone()).collect(),
        blank: region.sets.iter().flat_map(|s| s.blank.clone()).collect(),
    };
    if union.points.is_empty() {
        return RegionCall::Unknown;
    }
    let (px, py) = region.pitch;
    let xs: &[i32] = if px.is_some() { &[-1, 0, 1] } else { &[0] };
    let ys: &[i32] = if py.is_some() {
        &[-2, -1, 0, 1, 2]
    } else {
        &[0]
    };
    let base = shift_correlation(&union, page4, warp, PointPx::default());
    let mut best = (f64::NEG_INFINITY, PointPx::default());
    for &j in ys {
        for &i in xs {
            if (i, j) == (0, 0) {
                continue;
            }
            let c = PointPx {
                x: f64::from(i) * px.unwrap_or(0.0),
                y: f64::from(j) * py.unwrap_or(0.0),
            };
            let v = shift_correlation(&union, page4, warp, c);
            if v > best.0 {
                best = (v, c);
            }
        }
    }
    let m = f64::from(cfg.verify_margin_milli) / 1000.0;
    if best.0 - base >= m {
        RegionCall::Shift(best.1)
    } else if base - best.0 >= m {
        RegionCall::Stay
    } else {
        RegionCall::Unknown
    }
}

/// A warp refitted with every region moved by its best one-pitch shift: template bubble `t`
/// of a shifted region corresponds to `W(t − c)`; regions whose content decides nothing are
/// left out of the fit (ICP then places them). `None` when no region moves.
fn corrected_warp(
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
    warp: &QuadWarp,
) -> Option<QuadWarp> {
    let calls: Vec<RegionCall> = rm
        .regions
        .iter()
        .map(|r| region_call(pyr, cfg, r, warp))
        .collect();
    if !calls.iter().any(|c| matches!(c, RegionCall::Shift(_))) {
        return None;
    }
    let mut src = Vec::with_capacity(rm.points.len());
    let mut dst = Vec::with_capacity(rm.points.len());
    for (j, t) in rm.points.iter().enumerate() {
        let c = match rm.region_of.get(j).and_then(|&r| calls.get(r)) {
            Some(RegionCall::Stay) => PointPx::default(),
            Some(RegionCall::Shift(c)) => *c,
            _ => continue,
        };
        src.push(*t);
        dst.push(warp.map(PointPx {
            x: t.x - c.x,
            y: t.y - c.y,
        }));
    }
    if src.len() < cfg.icp_min_matches as usize {
        return None;
    }
    fit_quadratic_projective(
        &src,
        &dst,
        None,
        rm.centre,
        f64::from(cfg.quad_norm_px),
        warp.den,
    )
}

/// An accepted alignment.
struct Fit {
    warp: QuadWarp,
    q: RegistrationQuality,
    matched: Vec<bool>,
    exceptions: Vec<Exception>,
    /// Regions the verification could not test.
    untested: Vec<usize>,
    /// Bubbles of bands that failed the local verification.
    local: Vec<usize>,
}

/// The outcome of one registration path.
enum Attempt {
    Verified(Box<Fit>),
    /// Registered and gated, but no alignment was verified: refused unless another path is.
    Unverified {
        q: RegistrationQuality,
        detail: String,
    },
    /// A gate failed.
    Failed {
        q: RegistrationQuality,
        exception: Exception,
    },
}

/// One registration path: coarse, ICP, gates, verification, one correction. `row`: the
/// camera path's row-pitch vector (`None`: the scanner path).
fn attempt(
    pyr: &Pyramid,
    rm: &RefModel,
    cfg: &RegistrationConfig,
    lat: &Lattice,
    row: Option<PointPx>,
    q0: &RegistrationQuality,
) -> Attempt {
    let camera = row.is_some();
    let mut q = q0.clone();
    if camera {
        q.detections = lat.rings.len() as u32;
        q.ring_radius_px = lat.radius;
        q.pitch_px = (lat.v.x * lat.v.x + lat.v.y * lat.v.y).sqrt();
    }
    let failed = |q: RegistrationQuality, exception: Exception| Attempt::Failed { q, exception };
    // 4. Coarse.
    let tp = mpx(cfg.template_pitch_mpx);
    let pitch = (lat.v.x * lat.v.x + lat.v.y * lat.v.y).sqrt();
    let shape = if let Some(vr) = row {
        let (cx, cy) = (mpx(cfg.camera_pitch_mpx[0]), mpx(cfg.camera_pitch_mpx[1]));
        Shape::Linear {
            m: [lat.v.x / cx, vr.x / cy, lat.v.y / cx, vr.y / cy],
        }
    } else {
        Shape::Similarity {
            s0: pitch / tp,
            th0: lat.v.y.atan2(lat.v.x),
        }
    };
    let Some(co) = coarse(pyr, rm, shape, cfg) else {
        return failed(
            q,
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                "coarse alignment with the blank form failed".into(),
            ),
        );
    };
    q.coarse_ncc = co.ncc;
    q.alias_margin = co.alias_margin;
    q.orientation_margin = co.orientation_margin;
    let ncc = co.ncc;
    if ncc < f64::from(cfg.coarse_min_ncc_milli) / 1000.0 {
        return failed(
            q,
            reg_exc(
                "REG-001",
                Severity::Blocker,
                None,
                format!("page does not correlate with the blank form (NCC {ncc:.3})"),
            ),
        );
    }
    let orientation_margin = co.orientation_margin;
    if orientation_margin < f64::from(cfg.orientation_min_margin_milli) / 1000.0 {
        let detail = format!(
            "orientation undecided: the page correlates with the blank form about as well \
             upright as upside down (NCC margin {orientation_margin:.3}); rescan it upright"
        );
        return failed(q, reg_exc("REG-001", Severity::Blocker, None, detail));
    }
    // The camera path leaves the one-pitch question to the verification below.
    let alias_margin = co.alias_margin;
    if !camera && alias_margin < f64::from(cfg.alias_min_margin_milli) / 1000.0 {
        let detail =
            format!("alignment ambiguous by one lattice pitch (NCC margin {alias_margin:.3})");
        return failed(q, reg_exc("REG-003", Severity::Error, None, detail));
    }
    // 5. ICP.
    let warp = if camera {
        match icp_camera(rm, cfg, lat, &co.affine) {
            Some(w) => w,
            None => {
                return failed(
                    q,
                    reg_exc(
                        "REG-003",
                        Severity::Error,
                        None,
                        "registration failed: too few bubbles matched".into(),
                    ),
                );
            }
        }
    } else {
        icp_scanner(rm, cfg, lat, &co.affine)
    };
    let (matched, exceptions) = match assess(rm, cfg, lat, &warp, co.upside_down, &mut q) {
        Ok(x) => x,
        Err(e) => return failed(q, e),
    };
    // 6. Verification, then one correction.
    let v = verify(pyr, rm, cfg, lat, &warp);
    q.verify_margin = v.margin();
    q.verify_tests = v.tests.len() as u32;
    q.method = if camera {
        RegistrationMethod::Camera
    } else {
        RegistrationMethod::Lattice
    };
    q.window_margin = v.band_margin();
    q.window_floor = v.band_floor();
    if let (true, Some(detail)) = (v.passed(cfg), moved_detail(cfg, &v)) {
        q.windows_failed = v.moved_bands(cfg).len() as u32;
        return Attempt::Unverified { q, detail };
    }
    if v.passed(cfg) {
        let mut exceptions = exceptions;
        let local = local_check(rm, cfg, &v, &mut q, &mut exceptions);
        return Attempt::Verified(Box::new(Fit {
            warp,
            q,
            matched,
            exceptions,
            untested: v.untested,
            local,
        }));
    }
    let detail = v.describe(cfg);
    // Up to two corrections: a region the first refit could not place may be decided once the
    // others are right.
    let mut current = warp;
    for _ in 0..2 {
        let Some(start) = corrected_warp(pyr, rm, cfg, &current) else {
            break;
        };
        let Some(w2) = icp_quadratic(rm, cfg, lat, &|t| start.map(t), start.den) else {
            break;
        };
        current = w2;
        let mut q2 = q.clone();
        if let Ok((matched, mut exceptions)) = assess(rm, cfg, lat, &w2, co.upside_down, &mut q2) {
            let v2 = verify(pyr, rm, cfg, lat, &w2);
            q2.verify_margin = v2.margin();
            q2.verify_tests = v2.tests.len() as u32;
            if v2.passed(cfg)
                && let Some(detail) = moved_detail(cfg, &v2)
            {
                q2.window_margin = v2.band_margin();
                q2.windows_failed = v2.moved_bands(cfg).len() as u32;
                return Attempt::Unverified { q: q2, detail };
            }
            if v2.passed(cfg) {
                q2.method = if camera {
                    RegistrationMethod::CameraCorrected
                } else {
                    RegistrationMethod::LatticeCorrected
                };
                exceptions.push(reg_exc(
                    "REG-003",
                    Severity::Warn,
                    None,
                    format!(
                        "the first alignment was off by one bubble row or column in at least one \
                         region; corrected and verified against the blank form (margin {:.2})",
                        q2.verify_margin
                    ),
                ));
                let local = local_check(rm, cfg, &v2, &mut q2, &mut exceptions);
                return Attempt::Verified(Box::new(Fit {
                    warp: w2,
                    q: q2,
                    matched,
                    exceptions,
                    untested: v2.untested,
                    local,
                }));
            }
        }
    }
    Attempt::Unverified { q, detail }
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
        paper_normalised: pyr.background().is_some(),
        ..Default::default()
    };
    let fail = |q: RegistrationQuality, rings: Vec<RingHit>, e: Exception| Registration {
        warp: None,
        quality: q,
        rings,
        matched: vec![false; n_t],
        exceptions: vec![e],
        unverified: vec![false; n_t],
    };
    let lat = match lattice(page, pyr, rm, cfg, &mut q) {
        Ok(l) => l,
        Err(b) => {
            let (rings, e) = *b;
            return fail(q, rings, e);
        }
    };
    let accept = |f: Box<Fit>, lat: Lattice| Registration {
        warp: Some(f.warp),
        quality: f.q,
        rings: lat.rings,
        matched: f.matched,
        exceptions: f.exceptions,
        unverified: (0..n_t)
            .map(|j| {
                rm.region_of.get(j).is_some_and(|r| f.untested.contains(r))
                    || f.local.binary_search(&j).is_ok()
            })
            .collect(),
    };
    let first = match attempt(pyr, rm, cfg, &lat, None, &q) {
        Attempt::Verified(f) => {
            // A gray capture whose rings were mostly missed (a blurred or anisotropic photo):
            // the camera path's ring search may support more bubbles; keep the better verified
            // alignment. Bitonal scans never take this branch.
            let weak =
                u64::from(f.q.matched) * 1000 < u64::from(cfg.matched_warn_permille) * n_t as u64;
            if cfg.camera_path
                && weak
                && pyr.background().is_some()
                && let Some((cam, row)) = camera_lattice(page, pyr, rm, cfg, &lat)
                && let Attempt::Verified(g) = attempt(pyr, rm, cfg, &cam, Some(row), &q)
                && g.q.matched > f.q.matched
            {
                return accept(g, cam);
            }
            return accept(f, lat);
        }
        other => other,
    };
    let second = if cfg.camera_path {
        match camera_lattice(page, pyr, rm, cfg, &lat) {
            Some((cam, row)) => match attempt(pyr, rm, cfg, &cam, Some(row), &q) {
                Attempt::Verified(f) => return accept(f, cam),
                other => Some(other),
            },
            None => None,
        }
    } else {
        None
    };
    // Refuse: never a guess.
    let unverified = |q: RegistrationQuality, detail: String| {
        let e = reg_exc(
            "REG-003",
            Severity::Error,
            None,
            format!(
                "alignment with the blank form not verified (a bubble lattice can match one row \
                 or column away); nothing on this page is read: {detail}"
            ),
        );
        (q, e)
    };
    let (q, e) = match (first, second) {
        (Attempt::Unverified { q, detail }, _) | (_, Some(Attempt::Unverified { q, detail })) => {
            unverified(q, detail)
        }
        (Attempt::Failed { q, exception }, _) => (q, exception),
        (Attempt::Verified(_), _) => unreachable!("a verified attempt returned above"),
    };
    fail(q, lat.rings, e)
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
        let cfg = RegistrationConfig::default();
        let v = pitch_vector(&pts, &cfg).unwrap();
        let p = (v.x * v.x + v.y * v.y).sqrt();
        assert!((p / 50.0 - s).abs() < 1e-3, "{p}");
        assert!((v.y.atan2(v.x) - th).abs() < 2e-3);
        let r = row_pitch_vector(&pts, &cfg).unwrap();
        let pr = (r.x * r.x + r.y * r.y).sqrt();
        assert!((pr / 41.5 - s).abs() < 1e-3, "{pr}");
        assert!((r.x.atan2(r.y) + th).abs() < 2e-3);
    }

    /// The blank form under a known similarity, rendered as a gray page.
    fn synthetic_page(engine: &crate::LegacyEngine, truth: &Affine) -> GrayImage {
        let inv = truth.inverse().unwrap();
        GrayImage::from_fn(2400, 1700, |x, y| {
            let t = inv.map(PointPx {
                x: x as f64 + 0.5,
                y: y as f64 + 0.5,
            });
            bilinear(engine.blank(), t.x, t.y, 255)
        })
    }

    fn max_error(rm: &RefModel, w: &QuadWarp, truth: &Affine) -> f64 {
        rm.points
            .iter()
            .map(|t| {
                let (a, b) = (w.map(*t), truth.map(*t));
                ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
            })
            .fold(0.0, f64::max)
    }

    /// Adversarial alignments (§8.4, F1): the registered warp shifted by exactly one lattice
    /// pitch in every direction (and two rows). As shifted, most template bubbles still sit on
    /// rings, yet the verification never accepts such a warp. ICP started from it converges
    /// either back to the truth or onto an alias — the region-wise alias a phone photo
    /// produced (answers one answer row off, the header grids one header row off) — and every
    /// converged alias is rejected and corrected to the true alignment, which then verifies.
    /// Whatever verifies is the truth.
    #[test]
    fn one_pitch_aliases_are_never_verified_and_are_corrected() {
        let engine = crate::LegacyEngine::iau_ccsit_v1().unwrap();
        let rm = engine.rm_for_tests();
        let cfg = &engine.config().registration;
        let truth = Affine::similarity(0.97, 0.017, 30.0, 25.0);
        let page = synthetic_page(&engine, &truth);
        let pyr = Pyramid::new(&page);
        let mut q = RegistrationQuality::default();
        let lat = lattice(&page, &pyr, rm, cfg, &mut q).ok().unwrap();
        let reg = register_with(&page, &pyr, rm, cfg);
        let w = reg.warp.expect("registers");
        assert_eq!(reg.quality.method, RegistrationMethod::Lattice);
        assert!(max_error(rm, &w, &truth) < 1.0);
        let v = verify(&pyr, rm, cfg, &lat, &w);
        assert!(v.passed(cfg), "{}", v.describe(cfg));
        let mut converged_aliases = 0;
        let mut check = |name: &str, start: &QuadWarp| {
            let v = verify(&pyr, rm, cfg, &lat, start);
            if v.passed(cfg) {
                assert!(
                    max_error(rm, start, &truth) < 1.0,
                    "{name}: verified, not the truth"
                );
                return;
            }
            converged_aliases += 1;
            let c = corrected_warp(&pyr, rm, cfg, start).expect("a region moves");
            let fixed = icp_quadratic(rm, cfg, &lat, &|t| c.map(t), c.den).unwrap();
            let v = verify(&pyr, rm, cfg, &lat, &fixed);
            assert!(v.passed(cfg), "{name}: {}", v.describe(cfg));
            assert!(
                max_error(rm, &fixed, &truth) < 1.0,
                "{name}: corrected off the truth"
            );
        };
        for (i, j) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (0, 2),
            (0, -2),
        ] {
            let k = PointPx {
                x: f64::from(i) * 50.4,
                y: f64::from(j) * 41.62,
            };
            // alias(t) = W(t + k): every bubble read one pitch away.
            let alias = QuadWarp {
                centre: PointPx {
                    x: w.centre.x - k.x,
                    y: w.centre.y - k.y,
                },
                ..w
            };
            let name = format!("({i}, {j})");
            let on_rings = rm
                .points
                .iter()
                .filter(|t| lat.grid.nearest_within(alias.map(**t), 4.0).is_some())
                .count();
            assert!(on_rings >= 350, "{name}: {on_rings}");
            let v = verify(&pyr, rm, cfg, &lat, &alias);
            assert!(!v.passed(cfg), "{name}: a shifted warp was verified");
            assert!(v.margin() < 0.0, "{name}: margin {}", v.margin());
            // ICP from the shifted warp, as registration would run it.
            let converged =
                icp_quadratic(rm, cfg, &lat, &|t| alias.map(t), alias.den).expect("ICP");
            check(&name, &converged);
        }
        // The region-wise alias of the photo (F1): each region one of its own rows off.
        let (src, dst): (Vec<PointPx>, Vec<PointPx>) = rm
            .points
            .iter()
            .enumerate()
            .map(|(j, t)| {
                let py = rm.regions[rm.region_of[j]].pitch.1.unwrap_or(0.0);
                (
                    *t,
                    w.map(PointPx {
                        x: t.x,
                        y: t.y + py,
                    }),
                )
            })
            .unzip();
        let regional = fit_quadratic(&src, &dst, None, rm.centre, 1000.0).unwrap();
        check("region rows", &regional);
        assert!(converged_aliases >= 3, "{converged_aliases}");
    }

    #[test]
    fn similarity_shape_reproduces_the_brief_arithmetic() {
        let (s0, th0) = (0.9712, 0.0026);
        let sh = Shape::Similarity { s0, th0 };
        assert_eq!(sh.lin(false, None), Affine::similarity(s0, th0, 0.0, 0.0));
        assert_eq!(
            sh.lin(true, Some((985, -4_000))),
            Affine::similarity(
                s0 * 985.0 / 1000.0,
                th0 + std::f64::consts::PI + -4_000.0 * 1e-6,
                0.0,
                0.0
            )
        );
        // The linear shape applies the hypothesis in the template frame.
        let lin = Shape::Linear {
            m: [0.95, 0.0, 0.0, 0.99],
        };
        let a = lin.lin(false, Some((1_000, 0)));
        assert!((a.m[0] - 0.95).abs() < 1e-12 && (a.m[4] - 0.99).abs() < 1e-12);
        assert_eq!(lin.lin(true, None).m[0], -0.95);
    }
}
