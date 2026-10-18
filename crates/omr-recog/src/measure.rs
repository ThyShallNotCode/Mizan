//! Per-bubble measurement (brief steps 6–7; §8.4 Stage D pass 2, §8.6 inner disk).
//!
//! The registered centre of every template bubble snaps to a detected ring within 3 px (filled
//! bubbles hide their ring and keep the predicted centre). With `R_local = R₀ × local scale`
//! (the length of the warp's x-derivative), `inner` is the mean darkness `1 − v/255` over the
//! disk `r ≤ 0.72 R_local` and `core` over `r ≤ 0.45 R_local`, both quantised to
//! [`Darkness`] (1/1024); gray captures are read against their paper background `B` instead of
//! 255 (`D = (B − I)/B`, §8.5 with `K = 0`). A bubble is UNREADABLE — never empty — when registration failed, its
//! disk leaves the image (CAP-009), it is smaller than 20 px (CAP-002), or its local warp is not
//! trustworthy — too few matched bubbles nearby, or a local scale outside the 0.80–1.10 page
//! band (REG-003, Region).

use crate::config::{MeasureConfig, RegistrationConfig};
use crate::register::{RefModel, Registration};
use omr_imaging::{GrayImage, PointGrid, Warp, bilinear, disk_sum, disk_sum_paper};
use omr_model::{Darkness, PointPx};
use serde::{Deserialize, Serialize};

/// Why a bubble could not be observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unreadable {
    /// Page registration failed (REG-001/003/004 on the page).
    Registration,
    /// The local warp is not trustworthy: too few matched bubbles nearby, or a non-finite or
    /// implausible local scale (REG-003, Region).
    NoSupport,
    /// The measurement disk leaves the image: cut-off scan or torn corner (CAP-009).
    OutsideImage,
    /// The bubble is smaller than the minimum diameter (CAP-002).
    TooSmall,
}

impl Unreadable {
    /// The Appendix A code that explains this cause.
    pub fn code(self) -> &'static str {
        match self {
            Unreadable::Registration | Unreadable::NoSupport => "REG-003",
            Unreadable::OutsideImage => "CAP-009",
            Unreadable::TooSmall => "CAP-002",
        }
    }
}

/// One measured bubble.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BubbleMeasure {
    /// Measurement centre (page px).
    pub centre: PointPx,
    /// True if the centre snapped to a detected ring.
    pub snapped: bool,
    /// Local bubble radius `R₀ × local scale` (px).
    pub radius_px: f64,
    /// Inner-disk darkness.
    pub inner: Darkness,
    /// Core-disk darkness.
    pub core: Darkness,
    /// `Some(cause)` if the bubble is UNREADABLE (then `inner`/`core` are 0 and meaningless).
    pub unreadable: Option<Unreadable>,
}

impl BubbleMeasure {
    /// True if the bubble was observed.
    pub fn readable(&self) -> bool {
        self.unreadable.is_none()
    }
}

/// `background`: the paper level at 1/4 resolution for gray captures (darkness relative to
/// it, §8.5 with K = 0); `None` for bitonal captures (B = 255, the validated bitonal mode).
pub(crate) fn measure(
    page: &GrayImage,
    reg: &Registration,
    rm: &RefModel,
    rcfg: &RegistrationConfig,
    mcfg: &MeasureConfig,
    background: Option<&GrayImage>,
) -> Vec<BubbleMeasure> {
    let unread = |p: PointPx, cause| BubbleMeasure {
        centre: p,
        snapped: false,
        radius_px: 0.0,
        inner: Darkness(0),
        core: Darkness(0),
        unreadable: Some(cause),
    };
    let Some(warp) = reg.warp.as_ref() else {
        return rm
            .points
            .iter()
            .map(|_| unread(PointPx::default(), Unreadable::Registration))
            .collect();
    };
    let ring_pts: Vec<PointPx> = reg.rings.iter().map(|r| r.centre).collect();
    let rings = PointGrid::new(&ring_pts, 16.0);
    let matched_t: Vec<PointPx> = rm
        .points
        .iter()
        .zip(&reg.matched)
        .filter(|(_, m)| **m)
        .map(|(p, _)| *p)
        .collect();
    let support_r = f64::from(rcfg.support_radius_px);
    let support = PointGrid::new(&matched_t, support_r.max(16.0));
    let snap = f64::from(rcfg.snap_mpx) / 1000.0;
    let fi = f64::from(mcfg.inner_permille) / 1000.0;
    let fc = f64::from(mcfg.core_permille) / 1000.0;
    let min_d = f64::from(mcfg.min_diameter_mpx) / 1000.0;
    let band = if background.is_some() && reg.quality.scale.is_finite() && reg.quality.scale > 0.0 {
        reg.quality.scale
    } else {
        1.0
    };
    rm.points
        .iter()
        .enumerate()
        .map(|(j, t)| {
            let p = warp.map(*t);
            let px = warp.map(PointPx {
                x: t.x + 1.0,
                y: t.y,
            });
            let s = ((px.x - p.x).powi(2) + (px.y - p.y).powi(2)).sqrt();
            let r_local = rm.radius * s;
            if !(p.x.is_finite() && p.y.is_finite() && s.is_finite()) {
                return unread(PointPx::default(), Unreadable::NoSupport);
            }
            if 2.0 * r_local < min_d {
                return unread(p, Unreadable::TooSmall);
            }
            // A local scale outside the band a page may have at all means the warp is
            // extrapolating, not measuring: UNREADABLE (REG-003), never a reading. On a gray
            // capture (a photo: perspective and anisotropy) the band is relative to the page's
            // own scale.
            if s * 1000.0 < f64::from(rcfg.scale_warn_permille[0]) * band
                || s * 1000.0 > f64::from(rcfg.scale_warn_permille[1]) * band
            {
                return unread(p, Unreadable::NoSupport);
            }
            let mut n_support = 0u32;
            support.for_each_within(*t, support_r, |_, _| n_support += 1);
            if n_support < rcfg.min_support {
                return unread(p, Unreadable::NoSupport);
            }
            let (centre, snapped) = match rings.nearest_within(p, snap) {
                Some((k, _)) => (ring_pts[k], true),
                None => (p, false),
            };
            let paper = background
                .map(|bg| bilinear(bg, centre.x / 4.0, centre.y / 4.0, 255))
                .unwrap_or(255)
                .max(1);
            let inner = disk_sum_paper(page, centre.x, centre.y, fi * r_local, paper);
            let core = disk_sum_paper(page, centre.x, centre.y, fc * r_local, paper);
            if inner.clipped() || inner.count == 0 {
                return unread(centre, Unreadable::OutsideImage);
            }
            // Its lattice region could not be verified against the blank form (the page shows
            // too little of it): never read on an unverified alignment.
            if reg.unverified.get(j).copied().unwrap_or(false) {
                return unread(centre, Unreadable::NoSupport);
            }
            BubbleMeasure {
                centre,
                snapped,
                radius_px: r_local,
                inner: Darkness(inner.darkness_q10_of(paper)),
                core: Darkness(core.darkness_q10_of(paper)),
                unreadable: None,
            }
        })
        .collect()
}

/// Inner darkness of every template bubble on the blank form itself (the print prior β_p⁰).
pub(crate) fn prior_inner(blank: &GrayImage, rm: &RefModel, mcfg: &MeasureConfig) -> Vec<Darkness> {
    let fi = f64::from(mcfg.inner_permille) / 1000.0;
    rm.points
        .iter()
        .map(|p| Darkness(disk_sum(blank, p.x, p.y, fi * rm.radius).darkness_q10()))
        .collect()
}
