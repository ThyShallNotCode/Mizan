//! Versioned recognition configuration (§8.11 "config hash" in the engine manifest, §14.6).
//!
//! Every threshold is an integer so the canonical JSON (RFC 8785 subset, `omr_forms::hash`) and
//! its SHA-384 are stable: lengths in milli-pixels (`_mpx`) at the template's resolution, ratios
//! in per-mille, NCC scores in thousandths (`_milli`), angles in micro-radians (`_urad`) and
//! darkness in 1/1024 units (`omr_model::Darkness`). The defaults reproduce the Python prototype
//! validated on 143 real IAU sheets (`RECOGNITION_BRIEF.md`, "Algorithm validated"); values that
//! are not from the brief are marked *initial*.

use omr_forms::hash::{hex, sha384_canonical};
use serde::{Deserialize, Serialize};

/// The configuration version string stamped into the hash.
pub const LEGACY_CONFIG_VERSION: &str = "omr-recog/legacy-bitonal/1";

/// The whole legacy-form recognition configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyConfig {
    /// Version string ([`LEGACY_CONFIG_VERSION`] for the defaults).
    pub version: String,
    /// Registration (§8.4 adapted to forms without fiducials).
    pub registration: RegistrationConfig,
    /// Measurement regions (§8.6).
    pub measure: MeasureConfig,
    /// Per-position baseline β_p (§8.6).
    pub baseline: BaselineConfig,
    /// Per-question decision thresholds (§8.8, bitonal slice).
    pub decision: DecisionConfig,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self::bitonal_v1()
    }
}

impl LegacyConfig {
    /// The configuration validated on the real IAU sheets (bitonal 600 → 200 dpi captures).
    pub fn bitonal_v1() -> Self {
        Self {
            version: LEGACY_CONFIG_VERSION.to_owned(),
            registration: RegistrationConfig::default(),
            measure: MeasureConfig::default(),
            baseline: BaselineConfig::default(),
            decision: DecisionConfig::default(),
        }
    }

    /// Canonical JSON of this configuration.
    pub fn canonical_json(&self) -> String {
        omr_forms::hash::canonical_json(self).expect("config holds integers and strings only")
    }

    /// SHA-384 (hex) of the canonical JSON, for `EngineManifest::config_sha384`.
    pub fn sha384_hex(&self) -> String {
        hex(&sha384_canonical(self).expect("config holds integers and strings only"))
    }
}

/// Registration parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationConfig {
    /// Ring radius of the half-resolution candidate pass (full-resolution px).
    pub candidate_radius_mpx: u32,
    /// Extra candidate radii tried when the first pass finds fewer than `min_rings` peaks.
    pub fallback_candidate_radii_mpx: Vec<u32>,
    /// Candidate threshold on the half-resolution ring NCC (*initial*).
    pub candidate_ncc_milli: u32,
    /// Detection radius set; the one nearest `bubble radius × pitch scale` is used.
    pub radii_mpx: Vec<u32>,
    /// Gaussian ring width.
    pub ring_width_mpx: u32,
    /// Detection threshold on the full-resolution ring NCC.
    pub detect_ncc_milli: u32,
    /// Flat-window guard on the window variance (0–1 intensity scale), parts per million.
    pub flat_variance_ppm: u32,
    /// Non-maximum suppression window, as a multiple of the radius (per-mille).
    pub nms_window_permille: u32,
    /// Fewer detections than this fails registration (REG-001) (*initial*).
    pub min_rings: u32,
    /// Pitch search: minimum horizontal neighbour distance.
    pub pitch_min_mpx: u32,
    /// Pitch search: maximum horizontal neighbour distance.
    pub pitch_max_mpx: u32,
    /// Pitch search: maximum |dy| / dx (per-mille).
    pub pitch_dy_permille: u32,
    /// Pitch cluster radius around the modal vector.
    pub pitch_cluster_mpx: u32,
    /// The template's option pitch at scale 1.
    pub template_pitch_mpx: u32,
    /// Margin cropped from each side of the blank form for the coarse correlation (per-mille).
    pub coarse_crop_permille: u32,
    /// Translation search padding around the page for the coarse correlation (full-res px).
    pub coarse_pad_px: u32,
    /// Scale multipliers searched around the pitch scale (per-mille).
    pub coarse_scale_permille: Vec<u32>,
    /// Angle offsets searched around the pitch angle (µrad).
    pub coarse_angle_urad: Vec<i32>,
    /// A coarse NCC below this fails registration (REG-001) (*initial*).
    pub coarse_min_ncc_milli: u32,
    /// Lattice pitches (x, y) of the one-pitch alias check (template px).
    pub alias_pitch_mpx: [u32; 2],
    /// The aligned NCC must beat every one-pitch translation by this much, else REG-003 ERROR
    /// (*initial*).
    pub alias_min_margin_milli: u32,
    /// The chosen orientation (upright or upside down) must correlate better than the other
    /// by this much at 1/32, else the orientation is undecided: REG-001, never a guess (§8.3)
    /// (*initial*: the 143 real IAU pages separate by ≥ 0.51, synthetic sheets by ≥ 0.60; a
    /// sheet printed over its own upside-down copy by 0.11).
    #[serde(default)]
    pub orientation_min_margin_milli: u32,
    /// ICP nearest-neighbour thresholds, one per round.
    pub icp_thresholds_mpx: Vec<u32>,
    /// Rounds (from the first) that fit an affine by LMedS; later rounds fit the quadratic warp.
    pub icp_affine_rounds: u32,
    /// Fewer matches than this stops ICP.
    pub icp_min_matches: u32,
    /// Huber-like weight scale `w = 1 / max(1, r / huber)`.
    pub huber_mpx: u32,
    /// LMedS samples per affine round.
    pub lmeds_iterations: u32,
    /// LMedS seed.
    pub lmeds_seed: u64,
    /// Quadratic-warp normalisation (px).
    pub quad_norm_px: u32,
    /// Final match radius for the matched count and RMS.
    pub final_match_mpx: u32,
    /// A predicted centre snaps to a detected ring within this distance.
    pub snap_mpx: u32,
    /// Matched fraction below this: REG-003 WARN (per-mille of template bubbles) (*initial*).
    pub matched_warn_permille: u32,
    /// Matched fraction below this: registration failed, REG-003 ERROR (*initial*).
    pub matched_error_permille: u32,
    /// RMS above this: REG-003 WARN (0.25 mm at 200 dpi, §8.4 scanner gate).
    pub rms_warn_mpx: u32,
    /// RMS above this: registration failed, REG-003 ERROR (0.5 mm at 200 dpi).
    pub rms_error_mpx: u32,
    /// Print scale silently accepted (per-mille, inclusive bounds; §8.4).
    pub scale_silent_permille: [u32; 2],
    /// Print scale accepted with REG-004 WARN; outside it REG-004 ERROR and failure.
    pub scale_warn_permille: [u32; 2],
    /// Skew above this: REG-004 `skew` WARN (3°).
    pub skew_warn_urad: u32,
    /// Anisotropy above this: REG-004 `anisotropy` WARN (2 %).
    pub anisotropy_warn_permille: u32,
    /// Matched template points within this radius support a bubble's local warp.
    pub support_radius_px: u32,
    /// Fewer supporting matches: the bubble is UNREADABLE (REG-003 Region) (*initial*).
    pub min_support: u32,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            candidate_radius_mpx: 13_000,
            fallback_candidate_radii_mpx: vec![11_000, 15_500],
            candidate_ncc_milli: 300,
            radii_mpx: vec![11_500, 12_500, 13_500, 14_500, 15_500, 16_500],
            ring_width_mpx: 1_600,
            detect_ncc_milli: 400,
            flat_variance_ppm: 2_000,
            nms_window_permille: 1_600,
            min_rings: 60,
            pitch_min_mpx: 42_000,
            pitch_max_mpx: 56_000,
            pitch_dy_permille: 300,
            pitch_cluster_mpx: 2_500,
            template_pitch_mpx: 50_000,
            coarse_crop_permille: 80,
            coarse_pad_px: 240,
            coarse_scale_permille: vec![985, 995, 1_000, 1_005, 1_015],
            coarse_angle_urad: vec![-4_000, 0, 4_000],
            coarse_min_ncc_milli: 200,
            alias_pitch_mpx: [50_000, 41_500],
            alias_min_margin_milli: 50,
            orientation_min_margin_milli: 200,
            icp_thresholds_mpx: vec![12_000, 8_000, 6_000, 4_000, 3_000, 3_000],
            icp_affine_rounds: 2,
            icp_min_matches: 30,
            huber_mpx: 1_500,
            lmeds_iterations: 300,
            lmeds_seed: 0x6f6d_725f_6c6d_6564,
            quad_norm_px: 1_000,
            final_match_mpx: 4_000,
            snap_mpx: 3_000,
            matched_warn_permille: 700,
            matched_error_permille: 450,
            rms_warn_mpx: 1_969,
            rms_error_mpx: 3_937,
            scale_silent_permille: [880, 1_050],
            scale_warn_permille: [800, 1_100],
            skew_warn_urad: 52_360,
            anisotropy_warn_permille: 20,
            support_radius_px: 350,
            min_support: 2,
        }
    }
}

/// Measurement regions (§8.6, adapted: the brief's inner and core disks).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasureConfig {
    /// Inner disk radius as a fraction of the local bubble radius (per-mille).
    pub inner_permille: u32,
    /// Core disk radius (per-mille).
    pub core_permille: u32,
    /// Local bubble diameter below this: UNREADABLE, CAP-002 (20 px, §8.9).
    pub min_diameter_mpx: u32,
}

impl Default for MeasureConfig {
    fn default() -> Self {
        Self {
            inner_permille: 720,
            core_permille: 450,
            min_diameter_mpx: 20_000,
        }
    }
}

/// How a position without enough unfilled instances gets its baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaselineFallback {
    /// The median batch baseline of the position's field (the prototype's rule).
    FieldMedian,
    /// The blank-form print coverage β_p⁰ plus the field's median (batch − prior) offset.
    PriorPlusFieldOffset,
}

/// Per-position baseline β_p (§8.6, brief "Printed baseline").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineConfig {
    /// An instance counts as unfilled when its inner darkness is below this (0.45).
    pub unfilled_max: u16,
    /// Fewer unfilled instances than this: fall back.
    pub min_unfilled: u32,
    /// No baseline exceeds this (0.35).
    pub cap: u16,
    /// Fallback rule.
    pub fallback: BaselineFallback,
}

impl Default for BaselineConfig {
    fn default() -> Self {
        Self {
            unfilled_max: 461,
            min_unfilled: 5,
            cap: 358,
            fallback: BaselineFallback::PriorPlusFieldOffset,
        }
    }
}

/// When a confident multiple mark is routed to review (§8.8 `multi_review`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultiReview {
    /// Always review.
    Always,
    /// Review in high-stakes mode only.
    HighStakes,
    /// Auto-score confident multis (MRK-002 INFO).
    Never,
}

/// Group decision thresholds (brief "Decide per question"); darkness deltas δ = inner − β_p in
/// 1/1024 units, ratios n = δ / level in per-mille.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionConfig {
    /// Blank when top-1 δ is below this (0.08).
    pub blank_max: u16,
    /// Selected needs δ ≥ this (0.20) …
    pub select_abs: u16,
    /// … and n ≥ this (0.55).
    pub select_rel_permille: u16,
    /// Strong (multi) needs δ ≥ this (0.30) …
    pub strong_abs: u16,
    /// … and n ≥ this (0.80).
    pub strong_rel_permille: u16,
    /// Erasure rule: runner-up / top-1 ≤ this (0.5).
    pub erasure_rho_permille: u16,
    /// Erasure rule: top-1 − runner-up ≥ this (0.20).
    pub erasure_gap: u16,
    /// A runner-up above this logs MRK-003 `resolved_erasure` (0.10).
    pub residue_log: u16,
    /// Page mark level: median top-1 δ over questions with top-1 ≥ this (0.25) …
    pub level_clear_min: u16,
    /// … clamped below (0.30) …
    pub level_min: u16,
    /// … and above (1.0).
    pub level_max: u16,
    /// Hysteresis band h: a decision variable within h of its threshold routes to review
    /// (§8.7 step 7: 8/1024 for D features) (*initial*).
    pub hysteresis: u16,
    /// Multiplier on `hysteresis` for bitonal input (§8.7 step 7: doubled).
    pub bitonal_hysteresis_factor: u16,
    /// Confident-multi review policy.
    pub multi_review: MultiReview,
    /// The erasure rule is on (§8.8; review-only in high-stakes mode regardless).
    pub erasure_rule: bool,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            blank_max: 82,
            select_abs: 205,
            select_rel_permille: 550,
            strong_abs: 307,
            strong_rel_permille: 800,
            erasure_rho_permille: 500,
            erasure_gap: 205,
            residue_log: 102,
            level_clear_min: 256,
            level_min: 307,
            level_max: 1024,
            hysteresis: 8,
            bitonal_hysteresis_factor: 2,
            multi_review: MultiReview::Never,
            erasure_rule: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = LegacyConfig::bitonal_v1();
        assert_eq!(a.sha384_hex().len(), 96);
        assert_eq!(a.sha384_hex(), LegacyConfig::default().sha384_hex());
        let mut b = a.clone();
        b.decision.hysteresis += 1;
        assert_ne!(a.sha384_hex(), b.sha384_hex());
        assert!(a.canonical_json().starts_with("{\"baseline\":"));
    }
}
