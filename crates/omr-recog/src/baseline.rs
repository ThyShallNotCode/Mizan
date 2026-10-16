//! The per-position empty baseline β_p (§8.6; brief "Printed baseline").
//!
//! **Batch mode** (the batch stage of §8.1): β_p is the median inner darkness over the batch's
//! *unfilled* instances of position p (inner < 0.45), capped at 0.35. Positions with fewer than
//! 5 unfilled instances — the leading digits of student numbers are filled on almost every
//! sheet — fall back to the blank-form print coverage β_p⁰ plus the field's median
//! (batch − prior) offset (or, configurably, to the field's median baseline, the prototype's
//! rule), and the batch logs MRK-016 `prior_model` (INFO). Medians are taken over multisets, so
//! the result does not depend on page order (§8.11).
//!
//! **Single-page mode** uses the blank-form prior β_p⁰ only.

use crate::config::{BaselineConfig, BaselineFallback};
use crate::exc;
use crate::measure::BubbleMeasure;
use omr_model::{Darkness, Exception, Severity};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Where a position's baseline came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BaselineSource {
    /// Median of `n` unfilled batch instances.
    Batch {
        /// Unfilled instances used.
        n: u32,
    },
    /// Too few unfilled instances: the configured fallback.
    Fallback,
    /// The blank-form prior (single-page mode).
    Prior,
}

/// Baselines for every template position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// β_p per position (template order).
    pub beta: Vec<Darkness>,
    /// Source per position.
    pub source: Vec<BaselineSource>,
    /// Batch-level exceptions (MRK-016 `prior_model`).
    pub exceptions: Vec<Exception>,
}

/// Integer median, rounding half up for even counts.
pub(crate) fn median_u16(v: &mut [u16]) -> Option<u16> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (u32::from(v[n / 2 - 1]) + u32::from(v[n / 2])).div_ceil(2) as u16
    })
}

fn median_i32(v: &mut [i32]) -> Option<i32> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2] + 1).div_euclid(2)
    })
}

/// Single-page mode: the blank-form prior, capped.
pub(crate) fn prior_baseline(prior: &[Darkness], cfg: &BaselineConfig) -> Baseline {
    Baseline {
        beta: prior.iter().map(|b| Darkness(b.0.min(cfg.cap))).collect(),
        source: vec![BaselineSource::Prior; prior.len()],
        exceptions: Vec::new(),
    }
}

/// Batch mode over the measured pages (each a slice in template order).
pub(crate) fn batch_baseline(
    pages: &[&[BubbleMeasure]],
    prior: &[Darkness],
    fields: &[Range<usize>],
    cfg: &BaselineConfig,
) -> Baseline {
    let n = prior.len();
    let mut beta: Vec<Option<u16>> = vec![None; n];
    let mut source = vec![BaselineSource::Fallback; n];
    let mut buf: Vec<u16> = Vec::with_capacity(pages.len());
    for p in 0..n {
        buf.clear();
        for page in pages {
            if let Some(b) = page.get(p)
                && b.readable()
                && b.inner.0 < cfg.unfilled_max
            {
                buf.push(b.inner.0);
            }
        }
        if buf.len() as u32 >= cfg.min_unfilled.max(1) {
            let m = median_u16(&mut buf).unwrap_or(0).min(cfg.cap);
            beta[p] = Some(m);
            source[p] = BaselineSource::Batch {
                n: buf.len() as u32,
            };
        }
    }
    let mut fallbacks = 0usize;
    let mut out = vec![Darkness(0); n];
    for range in fields {
        let mut offsets: Vec<i32> = Vec::new();
        let mut values: Vec<u16> = Vec::new();
        for p in range.clone() {
            if let Some(b) = beta[p] {
                offsets.push(i32::from(b) - i32::from(prior[p].0));
                values.push(b);
            }
        }
        let off = median_i32(&mut offsets);
        let med = median_u16(&mut values);
        for p in range.clone() {
            out[p] = Darkness(match beta[p] {
                Some(b) => b,
                None => {
                    fallbacks += 1;
                    let v = match (cfg.fallback, off, med) {
                        (BaselineFallback::PriorPlusFieldOffset, Some(o), _) => {
                            (i32::from(prior[p].0) + o).clamp(0, 1024) as u16
                        }
                        (BaselineFallback::FieldMedian, _, Some(m)) => m,
                        _ => prior[p].0,
                    };
                    v.min(cfg.cap)
                }
            });
        }
    }
    let mut exceptions = Vec::new();
    if fallbacks > 0 {
        exceptions.push(
            exc::batch("MRK-016", Severity::Info)
                .with_sub_reason("prior_model")
                .with_detail(format!(
                    "{fallbacks} position(s) had fewer than {} unfilled instances; baseline from the {}",
                    cfg.min_unfilled,
                    match cfg.fallback {
                        BaselineFallback::PriorPlusFieldOffset =>
                            "blank-form print coverage plus the field offset",
                        BaselineFallback::FieldMedian => "field median",
                    }
                )),
        );
    }
    Baseline {
        beta: out,
        source,
        exceptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omr_model::PointPx;

    const ALL4: Range<usize> = 0..4;
    const ONE: Range<usize> = 0..1;

    fn bm(inner: u16) -> BubbleMeasure {
        BubbleMeasure {
            centre: PointPx::default(),
            snapped: true,
            radius_px: 13.5,
            inner: Darkness(inner),
            core: Darkness(inner),
            unreadable: None,
        }
    }

    #[test]
    fn always_filled_position_falls_back() {
        // Position 0 is filled on every page; positions 1–3 are empty with print ≈ 0.18.
        let pages: Vec<Vec<BubbleMeasure>> = (0..20)
            .map(|i| vec![bm(900 + i), bm(180 + i % 3), bm(190), bm(200 + i % 5)])
            .collect();
        let refs: Vec<&[BubbleMeasure]> = pages.iter().map(|p| p.as_slice()).collect();
        let prior = [Darkness(170), Darkness(170), Darkness(180), Darkness(190)];
        let cfg = BaselineConfig::default();
        let b = batch_baseline(&refs, &prior, std::slice::from_ref(&ALL4), &cfg);
        assert_eq!(b.source[0], BaselineSource::Fallback);
        assert!(matches!(b.source[1], BaselineSource::Batch { n: 20 }));
        // Prior 170 + median offset of {11, 10, 12} = 181; never the filled 0.9.
        assert_eq!(b.beta[0], Darkness(181));
        assert!(b.beta.iter().all(|d| d.0 <= cfg.cap));
        assert_eq!(b.exceptions[0].code, "MRK-016");
        let mut fm = cfg.clone();
        fm.fallback = BaselineFallback::FieldMedian;
        let b2 = batch_baseline(&refs, &prior, std::slice::from_ref(&ALL4), &fm);
        assert_eq!(b2.beta[0], Darkness(190));
        // Page order does not matter.
        let rev: Vec<&[BubbleMeasure]> = refs.iter().rev().copied().collect();
        assert_eq!(
            batch_baseline(&rev, &prior, std::slice::from_ref(&ALL4), &cfg),
            b
        );
    }

    #[test]
    fn baseline_is_capped_and_prior_mode() {
        let pages: Vec<Vec<BubbleMeasure>> = (0..10).map(|_| vec![bm(420)]).collect();
        let refs: Vec<&[BubbleMeasure]> = pages.iter().map(|p| p.as_slice()).collect();
        let cfg = BaselineConfig::default();
        let b = batch_baseline(&refs, &[Darkness(100)], std::slice::from_ref(&ONE), &cfg);
        assert_eq!(b.beta[0], Darkness(cfg.cap));
        let p = prior_baseline(&[Darkness(500)], &cfg);
        assert_eq!(p.beta[0], Darkness(cfg.cap));
        assert_eq!(p.source[0], BaselineSource::Prior);
    }
}
