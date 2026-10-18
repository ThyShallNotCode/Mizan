//! Curving and scaling transforms (spec §10.6), fitted at L4 step 4 and frozen as numbers.

use crate::num::{Exact, exact, from_f64_fixed, sqrt_floor, to_f64};
use num_rational::Ratio;
use num_traits::{Signed, Zero};
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};

/// Decimal places at which irrational fitted parameters (σ ratios, square roots) freeze.
pub const FREEZE_DECIMALS: u32 = 6;

/// A transform to fit on the cohort (targets, not yet numbers).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurveSpec {
    /// x′ = x% + (μ_t − μ).
    AdditiveShift {
        /// Target mean μ_t (%).
        target_mean: Exact,
    },
    /// x′ = x%·100/max_obs.
    ProportionalToTop,
    /// (μ, max_obs) ↦ (μ_t, 100).
    TwoPointLinear {
        /// Target mean μ_t (%).
        target_mean: Exact,
    },
    /// x′ = μ_t + σ_t·(x% − μ)/σ (population σ).
    ZScaling {
        /// Target mean.
        target_mean: Exact,
        /// Target SD.
        target_sd: Exact,
    },
    /// x′ = 10·√x%.
    Root,
    /// External x% ↦ x′ table, piecewise linear between points (Remark parity).
    Lookup(Vec<(Exact, Exact)>),
}

/// A fitted, frozen transform: re-applied unchanged to later key versions and sittings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrozenCurve {
    /// x′ = slope·x + intercept (covers additive, proportional, two-point and z-scaling).
    Linear {
        /// Slope.
        slope: Exact,
        /// Intercept (percentage points).
        intercept: Exact,
    },
    /// x′ = 10·√x, floored at [`FREEZE_DECIMALS`].
    Root,
    /// Piecewise-linear table (sorted by x).
    Lookup(Vec<(Exact, Exact)>),
}

fn sco016(detail: impl Into<String>) -> Exception {
    Exception::must("SCO-016", Severity::Error, Scope::Exam).with_detail(detail)
}

fn mean(v: &[Exact]) -> Exact {
    let s: Exact = v.iter().copied().sum();
    s / Ratio::from_integer(v.len() as i128)
}

impl CurveSpec {
    /// Fits on the step-3 values of eligible students (§10.1 step 4). A refusal is SCO-016.
    pub fn fit(&self, cohort: &[Exact]) -> Result<FrozenCurve, Exception> {
        let one = Exact::from_integer(1);
        let hundred = Exact::from_integer(100);
        if cohort.is_empty() && !matches!(self, CurveSpec::Root | CurveSpec::Lookup(_)) {
            return Err(sco016("no eligible students to fit the curve"));
        }
        let curve = match self {
            CurveSpec::AdditiveShift { target_mean } => FrozenCurve::Linear {
                slope: one,
                intercept: target_mean - mean(cohort),
            },
            CurveSpec::ProportionalToTop => {
                let max = cohort.iter().max().copied().unwrap_or_default();
                if !max.is_positive() || max >= hundred {
                    return Err(sco016(format!(
                        "proportional-to-top refused: max_obs = {}",
                        crate::num::decimal_string(&max, 2, Default::default())
                    )));
                }
                FrozenCurve::Linear {
                    slope: hundred / max,
                    intercept: Exact::zero(),
                }
            }
            CurveSpec::TwoPointLinear { target_mean } => {
                let mu = mean(cohort);
                let max = cohort.iter().max().copied().unwrap_or_default();
                if max == mu {
                    return Err(sco016("two-point linear refused: max_obs = μ"));
                }
                let slope = (hundred - target_mean) / (max - mu);
                FrozenCurve::Linear {
                    slope,
                    intercept: target_mean - slope * mu,
                }
            }
            CurveSpec::ZScaling {
                target_mean,
                target_sd,
            } => {
                let mu = mean(cohort);
                let var: Exact = cohort.iter().map(|x| (x - mu) * (x - mu)).sum::<Exact>()
                    / Ratio::from_integer(cohort.len() as i128);
                if var.is_zero() {
                    return Err(sco016("z-scaling refused: σ = 0"));
                }
                let sigma = to_f64(&var).sqrt();
                let slope = from_f64_fixed(to_f64(target_sd) / sigma, FREEZE_DECIMALS);
                FrozenCurve::Linear {
                    slope,
                    intercept: target_mean - slope * mu,
                }
            }
            CurveSpec::Root => FrozenCurve::Root,
            CurveSpec::Lookup(t) => {
                let mut t = t.clone();
                t.sort();
                if t.windows(2).any(|w| w[1].1 < w[0].1 || w[1].0 == w[0].0) {
                    return Err(sco016("lookup table is not monotone non-decreasing"));
                }
                if t.is_empty() {
                    return Err(sco016("empty lookup table"));
                }
                FrozenCurve::Lookup(t)
            }
        };
        curve.check_monotone(cohort)?;
        Ok(curve)
    }
}

impl FrozenCurve {
    /// Applies the frozen transform exactly.
    pub fn apply(&self, x: &Exact) -> Exact {
        match self {
            FrozenCurve::Linear { slope, intercept } => slope * x + intercept,
            FrozenCurve::Root => Exact::from_integer(10) * sqrt_floor(x, FREEZE_DECIMALS),
            FrozenCurve::Lookup(t) => {
                // An empty table (never produced by `fit`) is the identity, not a panic.
                let (Some(&first), Some(&last)) = (t.first(), t.last()) else {
                    return *x;
                };
                if *x <= first.0 {
                    return first.1;
                }
                if *x >= last.0 {
                    return last.1;
                }
                for w in t.windows(2) {
                    let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                    if *x >= x0 && *x <= x1 {
                        return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
                    }
                }
                last.1
            }
        }
    }

    /// Monotone non-decreasing on the sorted observed scores and on a 0.01 grid over [0, 100]
    /// (§10.6), else SCO-016.
    pub fn check_monotone(&self, observed: &[Exact]) -> Result<(), Exception> {
        if let FrozenCurve::Linear { slope, .. } = self {
            // Linear maps are monotone iff slope ≥ 0; no grid needed.
            return if slope.is_negative() {
                Err(sco016("curve slope is negative (non-monotone)"))
            } else {
                Ok(())
            };
        }
        let mut obs = observed.to_vec();
        obs.sort();
        let check = |xs: &mut dyn Iterator<Item = Exact>| -> Result<(), Exception> {
            let mut prev: Option<Exact> = None;
            for x in xs {
                let y = self.apply(&x);
                if let Some(p) = prev
                    && y < p
                {
                    return Err(sco016(format!(
                        "curve decreases near {}",
                        crate::num::decimal_string(&x, 2, Default::default())
                    )));
                }
                prev = Some(y);
            }
            Ok(())
        };
        check(&mut obs.into_iter())?;
        check(&mut (0..=10_000).map(|k| exact(k, 100)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(n: i128) -> Exact {
        Exact::from_integer(n)
    }

    #[test]
    fn fits_hand_oracle() {
        // Cohort 40, 50, 60, 70: μ = 55, max = 70, population σ = √125 ≈ 11.18034.
        let c = [e(40), e(50), e(60), e(70)];
        // Additive to μ_t = 65: shift +10.
        let f = CurveSpec::AdditiveShift { target_mean: e(65) }
            .fit(&c)
            .unwrap();
        assert_eq!(f.apply(&e(40)), e(50));
        // Proportional to top: ×100/70; 70 -> 100, 35 -> 50.
        let f = CurveSpec::ProportionalToTop.fit(&c).unwrap();
        assert_eq!(f.apply(&e(70)), e(100));
        assert_eq!(f.apply(&e(35)), e(50));
        // Two-point: (55, 70) -> (65, 100): slope (100 - 65)/(70 - 55) = 7/3,
        // intercept 65 - 7/3 * 55 = -190/3; 60 -> 420/3 - 190/3 = 230/3 (76.67); 70 -> 100.
        let f = CurveSpec::TwoPointLinear { target_mean: e(65) }
            .fit(&c)
            .unwrap();
        assert_eq!(f.apply(&e(60)), exact(230, 3));
        assert_eq!(f.apply(&e(70)), e(100));
        assert_eq!(f.apply(&e(55)), e(65));
        // z-scaling to (70, 10): slope 10/11.18034 = 0.894427 (frozen 6 dp).
        let f = CurveSpec::ZScaling {
            target_mean: e(70),
            target_sd: e(10),
        }
        .fit(&c)
        .unwrap();
        assert_eq!(
            f,
            FrozenCurve::Linear {
                slope: exact(894_427, 1_000_000),
                intercept: e(70) - exact(894_427, 1_000_000) * e(55)
            }
        );
        // Root: 10·√64 = 80.
        assert_eq!(FrozenCurve::Root.apply(&e(64)), e(80));
    }

    #[test]
    fn refusals_are_sco016() {
        let flat = [e(50), e(50)];
        assert_eq!(
            CurveSpec::ZScaling {
                target_mean: e(70),
                target_sd: e(10)
            }
            .fit(&flat)
            .unwrap_err()
            .code,
            "SCO-016"
        );
        assert!(CurveSpec::ProportionalToTop.fit(&[e(100)]).is_err());
        assert!(
            CurveSpec::TwoPointLinear { target_mean: e(60) }
                .fit(&flat)
                .is_err()
        );
        let bad = CurveSpec::Lookup(vec![(e(0), e(10)), (e(50), e(5))]);
        assert!(bad.fit(&flat).is_err());
    }

    #[test]
    fn lookup_interpolates() {
        let f = CurveSpec::Lookup(vec![(e(0), e(0)), (e(50), e(60)), (e(100), e(100))])
            .fit(&[])
            .unwrap();
        assert_eq!(f.apply(&e(25)), e(30));
        assert_eq!(f.apply(&e(75)), e(80));
        assert_eq!(f.apply(&e(120)), e(100));
    }
}
