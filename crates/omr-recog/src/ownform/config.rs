//! Versioned own-form recognition configuration (§8.11 "config hash", §14.6).
//!
//! Integers only, so the canonical JSON and its SHA-384 are stable across platforms: lengths
//! on the page in micrometres (`_um`), lengths in the image in milli-pixels (`_mpx`), ratios in
//! per-mille, angles in micro-radians and darkness in 1/1024 units. The decision and baseline
//! blocks reuse the shared core's types ([`DecisionConfig`], [`BaselineConfig`]); only their
//! values differ for 8-bit gray captures. Values not taken from the spec are marked *initial*:
//! they were chosen on a synthetic calibration corpus (seeds disjoint from the evaluation) and
//! need corpus calibration (§23.5) before GA.

use crate::config::{BaselineConfig, BaselineFallback, DecisionConfig, MultiReview};
use omr_forms::hash::{hex, sha384_canonical};
use serde::{Deserialize, Serialize};

/// The configuration version string stamped into the hash.
pub const OWNFORM_CONFIG_VERSION: &str = "omr-recog/ownform-gray/1";

/// The whole own-form recognition configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnFormConfig {
    /// Version string ([`OWNFORM_CONFIG_VERSION`] for the defaults).
    pub version: String,
    /// Corner-tag search (§8.3 step 1).
    pub tags: TagConfig,
    /// Stage A global fit and its gates (§8.4).
    pub registration: StageAConfig,
    /// Page-code decoding (§8.3 step 3).
    pub code: CodeConfig,
    /// Row-tick local refinement (§8.4 Stage C control points, simplified).
    pub ticks: TickConfig,
    /// Measurement regions and normalised darkness (§8.5, §8.6).
    pub measure: OwnMeasureConfig,
    /// Per-position baseline β_p (§8.6), shared core.
    pub baseline: BaselineConfig,
    /// Group decisions (§8.8), shared core, for 8-bit gray captures.
    pub decision: DecisionConfig,
    /// Group decisions for bitonal captures (CAP-003 `bitonal`; §8.5 bitonal mode is its own
    /// calibration slice): the shared core's thresholds validated on the 143 real bitonal IAU
    /// sheets (θ_sel at 0.55 · level). The gray thresholds must not be used there: on those
    /// sheets they auto-accept an erasure residue as an answer (1 silent error in 4,096).
    pub decision_bitonal: DecisionConfig,
    /// Classical review triggers standing in for the deferred CNN router (§8.7 step 3).
    pub triggers: TriggerConfig,
}

impl Default for OwnFormConfig {
    fn default() -> Self {
        Self::gray_v1()
    }
}

impl OwnFormConfig {
    /// The configuration for 8-bit gray scanner captures of the generated A4 sheets.
    pub fn gray_v1() -> Self {
        Self {
            version: OWNFORM_CONFIG_VERSION.to_owned(),
            tags: TagConfig::default(),
            registration: StageAConfig::default(),
            code: CodeConfig::default(),
            ticks: TickConfig::default(),
            measure: OwnMeasureConfig::default(),
            baseline: BaselineConfig {
                fallback: BaselineFallback::PriorPlusFieldOffset,
                ..BaselineConfig::default()
            },
            decision: gray_decision(),
            decision_bitonal: DecisionConfig::default(),
            triggers: TriggerConfig::default(),
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

/// Decision thresholds for normalised darkness D on 8-bit gray captures. The rule is the shared
/// core's ([`crate::decide_group`]); only the relative selection threshold is lower than the
/// bitonal default, because gray light fills keep their darkness (D ≈ 0.25–0.5) instead of
/// turning into stipple, and the page level of a dark pen sheet is ≈ 0.9 (*initial*).
pub fn gray_decision() -> DecisionConfig {
    DecisionConfig {
        blank_max: 82,
        select_abs: 205,
        select_rel_permille: 300,
        strong_abs: 307,
        strong_rel_permille: 700,
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

/// Review triggers on top of the shared decision rule. They only ever turn an auto-accepted
/// decision into a review (the proposed response is unchanged), like the §8.7 router does
/// when a bubble needs the CNN that the MVP defers (*initial*, chosen on calibration corpora).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// A marked bubble (δ ≥ the blank threshold) with at least this per-mille of annulus
    /// pixels at D > 0.35 has ink outside its outline, as a cross-out leaves: MRK-004 review
    /// (F6 > a_min).
    pub cross_annulus_permille: u16,
    /// A selected bubble at most this dark (δ, 1/1024) …
    pub tick_max_delta: u16,
    /// … whose dark fraction (D > 0.5) lies in this per-mille range …
    pub tick_dark_permille: [u16; 2],
    /// … and exceeds its mean darkness by at least the first and at most the second value
    /// (per-mille; a stroke is either black or paper, a light fill uniformly gray) is a tick
    /// or X: MRK-005 review.
    pub tick_dark_minus_inner_permille: [i16; 2],
    /// An erasure the shared rule resolved (MRK-003 `resolved_erasure`) whose runner-up is at
    /// least this dark (δ, 1/1024) could be a light fill, which §8.8 never treats as a residue:
    /// MRK-003 review.
    pub erasure_residue_max: u16,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            cross_annulus_permille: 45,
            tick_max_delta: 390,
            tick_dark_permille: [60, 450],
            tick_dark_minus_inner_permille: [-80, 60],
            erasure_residue_max: 175,
        }
    }
}

/// Corner-tag search.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagConfig {
    /// Side of the square search window in each image corner, on the page (§8.3 says 60 mm;
    /// 70 mm covers ±4° skew plus an 8 mm scan margin) (*initial*).
    pub window_um: u32,
    /// The detector's quad search runs on an image decimated so that the 6.4 mm black square
    /// spans about this many pixels (the spec's L1 search, §8.2).
    pub search_square_px: u32,
    /// Largest decimation factor.
    pub max_downscale: u32,
    /// Largest accepted Hamming distance of a decoded tag (36h11 corrects up to 3).
    pub max_hamming: u8,
    /// Tag set of answer pages (IDs 4k … 4k+3, §6.4).
    pub answer_tag_set: u16,
    /// Tag sets that are recognised but routed elsewhere (headers, calibration, proofs).
    pub known_tag_sets: Vec<u16>,
    /// Sub-pixel edge refinement: samples within this distance of a corner are skipped
    /// (per-mille of the edge length).
    pub edge_trim_permille: u32,
    /// Sub-pixel edge refinement: half-width of the crossing search along the normal (mpx).
    pub edge_search_mpx: u32,
    /// Edge points further than this from the fitted line are dropped and the line refitted.
    pub edge_outlier_mpx: u32,
}

impl Default for TagConfig {
    fn default() -> Self {
        Self {
            window_um: 70_000,
            search_square_px: 16,
            max_downscale: 4,
            max_hamming: 2,
            answer_tag_set: 0,
            known_tag_sets: vec![0, 1, 2, 3],
            edge_trim_permille: 180,
            edge_search_mpx: 3_000,
            edge_outlier_mpx: 600,
        }
    }
}

/// Stage A: global fit from the refined tag corners and its gates (§8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageAConfig {
    /// RANSAC inlier threshold on corner residuals (mpx).
    pub ransac_threshold_mpx: u32,
    /// RANSAC confidence (per-mille).
    pub ransac_confidence_permille: u32,
    /// RANSAC iteration cap.
    pub ransac_max_iterations: u32,
    /// RANSAC seed (§8.11: seeded).
    pub ransac_seed: u64,
    /// Leave-one-out: drop a tag whose RMS residual exceeds max(this × median, the warn gate).
    pub loo_factor_permille: u32,
    /// RMS anchor residual above this: REG-003 WARN (0.25 mm, scanner).
    pub rms_warn_um: u32,
    /// RMS anchor residual above this: REG-003 ERROR, sheet review (0.5 mm).
    pub rms_error_um: u32,
    /// Print scale silently accepted (per-mille, inclusive).
    pub scale_silent_permille: [u32; 2],
    /// Print scale accepted with REG-004 WARN; outside it REG-004 ERROR.
    pub scale_warn_permille: [u32; 2],
    /// Skew above this: REG-004 `skew` WARN (3°).
    pub skew_warn_urad: u32,
    /// Anisotropy above this: REG-004 `anisotropy` WARN (2 %).
    pub anisotropy_warn_permille: u32,
    /// Header DPIs accepted as the capture resolution (§8.2 rule 2).
    pub standard_dpis: Vec<u32>,
}

impl Default for StageAConfig {
    fn default() -> Self {
        Self {
            ransac_threshold_mpx: 1_500,
            ransac_confidence_permille: 999,
            ransac_max_iterations: 500,
            ransac_seed: 0x6f6d_725f_7461_6773,
            loo_factor_permille: 3_000,
            rms_warn_um: 250,
            rms_error_um: 500,
            scale_silent_permille: [880, 1_050],
            scale_warn_permille: [800, 1_100],
            skew_warn_urad: 52_360,
            anisotropy_warn_permille: 20,
            standard_dpis: vec![150, 200, 240, 300, 400, 600],
        }
    }
}

/// Page-code decoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeConfig {
    /// Margin added around the symbol when cropping: the 3 mm quiet zone plus registration
    /// slack.
    pub crop_margin_um: u32,
    /// Retry a failed copy on a 2× bilinear upsample (§6.4 "2× retry").
    pub retry_upsample: bool,
}

impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            crop_margin_um: 4_500,
            retry_upsample: true,
        }
    }
}

/// Row-tick refinement: each 1.2 × 0.6 mm tick is located by its dark-mass centroid and the
/// row's bubbles are shifted by the residual interpolated between its two ticks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickConfig {
    /// Apply tick corrections to bubble centres.
    pub apply: bool,
    /// Search half-width around the predicted tick centre (µm on the page).
    pub search_um: u32,
    /// Corrections are applied only when the page's median tick residual exceeds this (µm);
    /// below it the Stage A fit is already better than the tick centroids (*initial*).
    pub min_median_residual_um: u32,
    /// A tick whose residual deviates from its column neighbours' median by more than this
    /// is ignored (µm) (*initial*).
    pub outlier_um: u32,
}

impl Default for TickConfig {
    fn default() -> Self {
        Self {
            apply: true,
            search_um: 900,
            min_median_residual_um: 60,
            outlier_um: 150,
        }
    }
}

/// Measurement regions (§8.6) and normalised darkness (§8.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnMeasureConfig {
    /// Inner disk radius, per-mille of r₀ (§8.6: 0.8, locked).
    pub inner_permille: u32,
    /// Core disk radius, per-mille of r₀.
    pub core_permille: u32,
    /// Paper background B: median over the annulus between these radii (per-mille of r₀).
    pub annulus_permille: [u32; 2],
    /// Local bubble diameter below this: UNREADABLE, CAP-002 (20 px, §8.9).
    pub min_diameter_mpx: u32,
    /// Contrast B − K below this: CAP-005 WARN (§8.9: 80).
    pub contrast_warn: u8,
    /// Contrast B − K below this: CAP-005 ERROR, sheet review (§8.9: 50).
    pub contrast_error: u8,
    /// Prior β_p⁰ used when no blank render was supplied (1/1024) (*initial*).
    pub default_prior: u16,
    /// Streak detection: a column band whose median offset from its ±4 px neighbours is at
    /// least this many gray levels (CAP-006) (*initial*).
    pub streak_min_delta: u8,
    /// Streak detection: widest band (px; §8.9: 1–4 px).
    pub streak_max_width: u32,
    /// Streak detection: row sampling step.
    pub streak_row_step: u32,
}

impl Default for OwnMeasureConfig {
    fn default() -> Self {
        Self {
            inner_permille: 800,
            core_permille: 450,
            annulus_permille: [1_200, 1_450],
            min_diameter_mpx: 20_000,
            contrast_warn: 80,
            contrast_error: 50,
            default_prior: 60,
            streak_min_delta: 12,
            streak_max_width: 4,
            streak_row_step: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = OwnFormConfig::gray_v1();
        assert_eq!(a.sha384_hex().len(), 96);
        assert_eq!(a.sha384_hex(), OwnFormConfig::default().sha384_hex());
        let mut b = a.clone();
        b.tags.window_um += 1;
        assert_ne!(a.sha384_hex(), b.sha384_hex());
    }
}
