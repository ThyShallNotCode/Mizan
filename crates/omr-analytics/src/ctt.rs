//! Classical test theory statistics (spec §12.1) with the small-N tiers of §12.3.
//!
//! Item statistics (p, D, r_pb, r_ir, option r, distractor trace lines) use u_i and the
//! number-right rest score R_i, so guessing penalties never enter item analysis; α, SEM and
//! CSEM use the graded x_i. Variances are population variances (÷N) (§10.2). `f64`,
//! advisory only.

use crate::data::{AnalysisData, CellStatus};
use omr_scoring::stats::{f_quantile, fisher_z_ci, percentile_sorted};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Small-N tiers by the number of examinees reaching the item (§12.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SmallN {
    /// N < 10: counts, p and option distribution only (SCO-023).
    Minimal,
    /// 10–29: low-confidence r_ir and α; rules 1–2 "possible — insufficient data"; rules 3–4
    /// suppressed (SCO-023).
    Low,
    /// 30–49: D hidden; rules 3–4 low-confidence.
    Limited,
    /// 50–99: all CTT statistics; Feldt and bootstrap CIs always shown.
    Moderate,
    /// ≥ 100: all statistics.
    Full,
}

impl SmallN {
    /// Tier of `n`.
    pub fn of(n: usize) -> Self {
        match n {
            0..=9 => SmallN::Minimal,
            10..=29 => SmallN::Low,
            30..=49 => SmallN::Limited,
            50..=99 => SmallN::Moderate,
            _ => SmallN::Full,
        }
    }
}

/// Difficulty flags (§12.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DifficultyFlag {
    /// p < 0.30.
    Hard,
    /// p > 0.90.
    Easy,
    /// p ≤ 1/m_i + 0.05: review.
    NearChance,
}

/// Discrimination bands of r_ir (§12.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Discrimination {
    /// r < 0: probable flaw or mis-key.
    ProbableFlaw,
    /// r < .10: review.
    Review,
    /// .10–.19.
    Marginal,
    /// .20–.29.
    Acceptable,
    /// ≥ .30.
    Good,
}

impl Discrimination {
    /// Band of r.
    pub fn of(r: f64) -> Self {
        if r < 0.0 {
            Discrimination::ProbableFlaw
        } else if r < 0.10 {
            Discrimination::Review
        } else if r < 0.20 {
            Discrimination::Marginal
        } else if r < 0.30 {
            Discrimination::Acceptable
        } else {
            Discrimination::Good
        }
    }
}

/// Option-level statistics (§12.1 option r, distractor functioning).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionStats {
    /// Master option, `None` = omit.
    pub option: Option<u8>,
    /// A keyed option.
    pub keyed: bool,
    /// Examinees choosing it (among those reaching the item).
    pub count: usize,
    /// P(o).
    pub proportion: f64,
    /// r_o = corr(1[chose o], R_i), `None` below N = 10 or with zero variance.
    pub r: Option<f64>,
    /// Proportion choosing it in the upper 27 % (number-right total).
    pub upper: f64,
    /// Proportion choosing it in the lower 27 %.
    pub lower: f64,
    /// Trace line: proportion choosing it per rest-score quintile (lowest first).
    pub trace: [f64; 5],
    /// Distractor with P(o) < 5 % (unverified rule).
    pub non_functioning: bool,
}

/// One item's statistics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemStats {
    /// Index in [`AnalysisData::items`].
    pub item: usize,
    /// Label.
    pub label: String,
    /// Examinees presented the item who reached it.
    pub n: usize,
    /// Small-N tier.
    pub tier: SmallN,
    /// Difficulty p = mean(u).
    pub p: Option<f64>,
    /// Difficulty flags.
    pub difficulty_flags: Vec<DifficultyFlag>,
    /// Upper–lower D (hidden below N = 50).
    pub d_index: Option<f64>,
    /// Uncorrected point-biserial (includes the item; Remark parity).
    pub r_pb: Option<f64>,
    /// Corrected item–rest r_ir (primary).
    pub r_ir: Option<f64>,
    /// Fisher-z 95 % CI of r_ir.
    pub r_ir_ci: Option<(f64, f64)>,
    /// Discrimination band.
    pub discrimination: Option<Discrimination>,
    /// r_ir and α carry a "low confidence" badge (10 ≤ N < 30).
    pub low_confidence: bool,
    /// Option statistics (choice and multi-select items), options then omit.
    pub options: Vec<OptionStats>,
}

/// Distribution summary of total scores (§12.1 "Summary").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// N.
    pub n: usize,
    /// Mean.
    pub mean: f64,
    /// Median.
    pub median: f64,
    /// Population SD; `None` ("n/a") for zero variance.
    pub sd: Option<f64>,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// 10th, 25th, 75th, 90th percentiles.
    pub percentiles: [f64; 4],
}

/// Reliability band (Downing 2004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlphaBand {
    /// ≥ .90: high stakes.
    HighStakes,
    /// .80–.89: end of course.
    EndOfCourse,
    /// .70–.79: low stakes.
    LowStakes,
    /// Below .70.
    Below,
}

/// Test statistics of one item-set class (identical master-item sets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestStats {
    /// Items of the class (indices into the data).
    pub items: Vec<usize>,
    /// Examinees in the class.
    pub n: usize,
    /// Graded total X = Σ x_i (not reached = 0).
    pub summary: Summary,
    /// KR-20 (all items {0, 1}, equal M_i, no penalty).
    pub kr20: Option<f64>,
    /// KR-21 (same condition).
    pub kr21: Option<f64>,
    /// Cronbach α on x_i.
    pub alpha: Option<f64>,
    /// Feldt 95 % CI for α.
    pub alpha_ci: Option<(f64, f64)>,
    /// α-if-item-deleted per item of the class (same order as `items`).
    pub alpha_if_deleted: Vec<Option<f64>>,
    /// Items whose deletion raises α by ≥ 0.01 (heuristic).
    pub alpha_rise_items: Vec<usize>,
    /// SEM = σ_X·√(1 − α).
    pub sem: Option<f64>,
    /// Reliability band.
    pub band: Option<AlphaBand>,
    /// Low-confidence badge (N < 30).
    pub low_confidence: bool,
}

/// Lord's binomial CSEM √(x(k − x)/(k − 1)) at number-right score x on k unit-weighted
/// dichotomous items (§12.1): k = 50, x = 30 → 3.5.
pub fn csem_lord(x: f64, k: usize) -> Option<f64> {
    (k >= 2 && (0.0..=k as f64).contains(&x))
        .then(|| (x * (k as f64 - x) / (k as f64 - 1.0)).sqrt())
}

/// Pearson correlation (population moments); `None` with zero variance or n < 2.
pub fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    let n = a.len();
    if n < 2 || b.len() != n {
        return None;
    }
    let (ma, mb) = (
        a.iter().sum::<f64>() / n as f64,
        b.iter().sum::<f64>() / n as f64,
    );
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - ma, y - mb);
        sab += dx * dy;
        saa += dx * dx;
        sbb += dy * dy;
    }
    (saa > 1e-12 && sbb > 1e-12).then(|| sab / (saa * sbb).sqrt())
}

fn pop_var(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let m = v.iter().sum::<f64>() / v.len() as f64;
    v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64
}

/// Cronbach α from item-score columns (population variances); `None` for k < 2 or zero
/// total variance ("n/a", never 0).
pub fn cronbach_alpha(columns: &[Vec<f64>]) -> Option<f64> {
    let k = columns.len();
    if k < 2 {
        return None;
    }
    let n = columns[0].len();
    let totals: Vec<f64> = (0..n).map(|e| columns.iter().map(|c| c[e]).sum()).collect();
    let vt = pop_var(&totals);
    if vt <= 1e-12 {
        return None;
    }
    let sv: f64 = columns.iter().map(|c| pop_var(c)).sum();
    Some(k as f64 / (k as f64 - 1.0) * (1.0 - sv / vt))
}

/// Feldt CI for α (γ = .05): [1 − (1 − α̂)·F_{.975}, 1 − (1 − α̂)·F_{.025}] with
/// F(N − 1, (N − 1)(k − 1)) (§12.1).
pub fn feldt_ci(alpha: f64, n: usize, k: usize) -> Option<(f64, f64)> {
    if n < 2 || k < 2 {
        return None;
    }
    let d1 = (n - 1) as f64;
    let d2 = d1 * (k - 1) as f64;
    Some((
        1.0 - (1.0 - alpha) * f_quantile(0.975, d1, d2),
        1.0 - (1.0 - alpha) * f_quantile(0.025, d1, d2),
    ))
}

fn summary(v: &[f64]) -> Summary {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    let mean = if n == 0 {
        0.0
    } else {
        s.iter().sum::<f64>() / n as f64
    };
    let var = pop_var(&s);
    Summary {
        n,
        mean,
        median: percentile_sorted(&s, 0.5),
        sd: (var > 1e-12).then(|| var.sqrt()),
        min: s.first().copied().unwrap_or(f64::NAN),
        max: s.last().copied().unwrap_or(f64::NAN),
        percentiles: [
            percentile_sorted(&s, 0.10),
            percentile_sorted(&s, 0.25),
            percentile_sorted(&s, 0.75),
            percentile_sorted(&s, 0.90),
        ],
    }
}

/// Upper and lower 27 % groups by `totals` (Kelley), ties at a cut all included.
pub fn upper_lower(totals: &[f64], fraction: f64) -> (Vec<bool>, Vec<bool>) {
    let n = totals.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut s = totals.to_vec();
    s.sort_by(f64::total_cmp);
    let g = ((n as f64 * fraction).ceil() as usize).clamp(1, n);
    let lo_cut = s[g - 1];
    let hi_cut = s[n - g];
    (
        totals.iter().map(|&t| t >= hi_cut).collect(),
        totals.iter().map(|&t| t <= lo_cut).collect(),
    )
}

/// Per-examinee number-right total Σ M_j·u_j over presented items (not reached = 0) and
/// presented maximum Σ M_j.
pub fn number_right(data: &AnalysisData) -> (Vec<f64>, Vec<f64>) {
    let mut tot = vec![0.0; data.n];
    let mut max = vec![0.0; data.n];
    for (i, it) in data.items.iter().enumerate() {
        for e in 0..data.n {
            let idx = data.idx(e, i);
            if data.status[idx] != CellStatus::NotPresented {
                tot[e] += it.max * data.u[idx];
                max[e] += it.max;
            }
        }
    }
    (tot, max)
}

/// Mean, population SD.
fn moments(v: &[f64]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    (
        m,
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n).sqrt(),
    )
}

/// Order-preserving integer key of an f64 (for fast, deterministic sorting).
fn sort_key(x: f64) -> u64 {
    let b = x.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

/// Item statistics for every item (§12.1), with the §12.3 tiers applied. One pass per item
/// accumulates option counts, rest-score sums, upper/lower and quintile counts; option r is
/// the point-biserial from those moments: r_o = (E[1_o·R] − p_o·E[R]) / (√(p_o(1 − p_o))·σ_R).
pub fn item_stats(data: &AnalysisData, upper_fraction: f64) -> Vec<ItemStats> {
    let (nr, nr_max) = number_right(data);
    let (upper, lower) = upper_lower(&nr, upper_fraction);
    let mut out = Vec::with_capacity(data.items.len());
    let mut keys: Vec<u128> = Vec::with_capacity(data.n);
    for (i, it) in data.items.iter().enumerate() {
        let reached: Vec<usize> = (0..data.n)
            .filter(|&e| data.status[data.idx(e, i)] == CellStatus::Reached)
            .collect();
        let n = reached.len();
        let tier = SmallN::of(n);
        let u: Vec<f64> = reached.iter().map(|&e| data.u[data.idx(e, i)]).collect();
        let p = (n > 0).then(|| u.iter().sum::<f64>() / n as f64);
        let mut difficulty_flags = Vec::new();
        if let Some(p) = p {
            if p < 0.30 {
                difficulty_flags.push(DifficultyFlag::Hard);
            }
            if p > 0.90 {
                difficulty_flags.push(DifficultyFlag::Easy);
            }
            if it.options > 0 && p <= 1.0 / f64::from(it.options) + 0.05 {
                difficulty_flags.push(DifficultyFlag::NearChance);
            }
        }
        // Rest score as a share of the examinee's presented maximum minus M_i (equals the
        // raw rest score up to a constant within one item-set class).
        let rest: Vec<f64> = reached
            .iter()
            .map(|&e| {
                let den = nr_max[e] - it.max;
                if den > 0.0 {
                    (nr[e] - it.max * data.u[data.idx(e, i)]) / den
                } else {
                    0.0
                }
            })
            .collect();
        let totals: Vec<f64> = reached.iter().map(|&e| nr[e]).collect();
        let show_r = tier >= SmallN::Low;
        let r_ir = if show_r { pearson(&u, &rest) } else { None };
        let r_pb = if show_r { pearson(&u, &totals) } else { None };
        let r_ir_ci = r_ir.and_then(|r| fisher_z_ci(r, n as u64));
        let (mut up_n, mut lo_n, mut up_u, mut lo_u) = (0usize, 0usize, 0.0, 0.0);
        for (k, &e) in reached.iter().enumerate() {
            if upper[e] {
                up_n += 1;
                up_u += u[k];
            }
            if lower[e] {
                lo_n += 1;
                lo_u += u[k];
            }
        }
        let d_index = (tier >= SmallN::Moderate && up_n > 0 && lo_n > 0)
            .then(|| up_u / up_n as f64 - lo_u / lo_n as f64);
        // Rank-based rest-score quintiles, ties by examinee order: group ⌊rank·5/N⌋. Group q
        // starts at rank ⌈q·N/5⌉, so four O(N) selections of packed (value, index) keys give
        // the cut points without a full sort.
        let packed = |k: usize| (u128::from(sort_key(rest[k])) << 64) | k as u128;
        keys.clear();
        keys.extend((0..n).map(packed));
        let mut cuts = [u128::MAX; 4];
        for (q, cut) in cuts.iter_mut().enumerate() {
            let r = ((q + 1) * n).div_ceil(5);
            if r < n {
                *cut = *keys.select_nth_unstable(r).1;
            }
        }
        let quintile: Vec<usize> = (0..n)
            .map(|k| {
                let key = packed(k);
                cuts.iter().filter(|c| key >= **c).count()
            })
            .collect();
        let mut options = Vec::new();
        if it.options > 0 {
            let m = usize::from(it.options);
            let cats = m + 1; // options, then omit
            let mut count = vec![0usize; cats];
            let mut sum_rest = vec![0.0f64; cats];
            let mut up_c = vec![0usize; cats];
            let mut lo_c = vec![0usize; cats];
            let mut trace_c = vec![[0usize; 5]; cats];
            let mut sizes = [0usize; 5];
            for (k, &e) in reached.iter().enumerate() {
                sizes[quintile[k]] += 1;
                let c = data.choice[data.idx(e, i)];
                let mut add = |cat: usize| {
                    count[cat] += 1;
                    sum_rest[cat] += rest[k];
                    up_c[cat] += usize::from(upper[e]);
                    lo_c[cat] += usize::from(lower[e]);
                    trace_c[cat][quintile[k]] += 1;
                };
                if c == 0 {
                    add(m);
                } else {
                    let mut bits = c;
                    while bits != 0 {
                        let o = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        if o < m {
                            add(o);
                        }
                    }
                }
            }
            let (mean_r, sd_r) = moments(&rest);
            let nf = n as f64;
            for cat in 0..cats {
                let option = (cat < m).then_some(cat as u8);
                let prop = if n > 0 { count[cat] as f64 / nf } else { 0.0 };
                let r = (show_r && sd_r > 1e-12 && prop > 0.0 && prop < 1.0).then(|| {
                    (sum_rest[cat] / nf - prop * mean_r) / ((prop * (1.0 - prop)).sqrt() * sd_r)
                });
                let mut trace = [0.0; 5];
                for q in 0..5 {
                    if sizes[q] > 0 {
                        trace[q] = trace_c[cat][q] as f64 / sizes[q] as f64;
                    }
                }
                let keyed = option.is_some_and(|o| it.keyed & (1 << o) != 0);
                options.push(OptionStats {
                    option,
                    keyed,
                    count: count[cat],
                    proportion: prop,
                    r,
                    upper: if up_n > 0 {
                        up_c[cat] as f64 / up_n as f64
                    } else {
                        0.0
                    },
                    lower: if lo_n > 0 {
                        lo_c[cat] as f64 / lo_n as f64
                    } else {
                        0.0
                    },
                    trace,
                    non_functioning: option.is_some() && !keyed && n > 0 && prop < 0.05,
                });
            }
        }
        out.push(ItemStats {
            item: i,
            label: it.label.clone(),
            n,
            tier,
            p,
            difficulty_flags,
            d_index,
            r_pb,
            r_ir,
            r_ir_ci,
            discrimination: r_ir.map(Discrimination::of),
            low_confidence: tier == SmallN::Low,
            options,
        });
    }
    out
}

/// Test statistics per item-set class (§12, "Pooling"): reliability uses the complete N × k
/// graded matrix of the class, not reached = 0.
pub fn test_stats(data: &AnalysisData) -> Vec<TestStats> {
    let words = data.items.len().div_ceil(64);
    let mut keys = vec![vec![0u64; words]; data.n];
    for i in 0..data.items.len() {
        for (e, key) in keys.iter_mut().enumerate() {
            if data.status[data.idx(e, i)] != CellStatus::NotPresented {
                key[i / 64] |= 1 << (i % 64);
            }
        }
    }
    let mut classes: BTreeMap<Vec<u64>, Vec<usize>> = BTreeMap::new();
    for (e, key) in keys.into_iter().enumerate() {
        if key.iter().any(|w| *w != 0) {
            classes.entry(key).or_default().push(e);
        }
    }
    let mut out = Vec::new();
    for (key, rows) in classes {
        let items: Vec<usize> = (0..data.items.len())
            .filter(|i| key[i / 64] & (1 << (i % 64)) != 0)
            .collect();
        let n = rows.len();
        let cols: Vec<Vec<f64>> = items
            .iter()
            .map(|&i| {
                rows.iter()
                    .map(|&e| {
                        let idx = data.idx(e, i);
                        if data.status[idx] == CellStatus::NotReached {
                            0.0
                        } else {
                            data.x[idx]
                        }
                    })
                    .collect()
            })
            .collect();
        let totals: Vec<f64> = (0..n).map(|r| cols.iter().map(|c| c[r]).sum()).collect();
        let k = items.len();
        let vt = pop_var(&totals);
        let dichotomous = !data.penalties
            && items
                .windows(2)
                .all(|w| data.items[w[0]].max == data.items[w[1]].max)
            && cols
                .iter()
                .zip(&items)
                .all(|(c, &i)| c.iter().all(|v| *v == 0.0 || *v == data.items[i].max));
        let (kr20, kr21) = if dichotomous && k >= 2 && vt > 1e-12 {
            let m = data.items[items[0]].max;
            let spq: f64 = cols
                .iter()
                .map(|c| {
                    let p = c.iter().sum::<f64>() / (n as f64 * m);
                    p * (1.0 - p)
                })
                .sum();
            let kf = k as f64;
            // In item units (number right), σ² scales by 1/m².
            let var_nr = vt / (m * m);
            let mu = totals.iter().sum::<f64>() / (n as f64 * m);
            (
                Some(kf / (kf - 1.0) * (1.0 - spq / var_nr)),
                Some(kf / (kf - 1.0) * (1.0 - mu * (kf - mu) / (kf * var_nr))),
            )
        } else {
            (None, None)
        };
        let alpha = cronbach_alpha(&cols);
        // α-if-deleted without copying: var(X − x_i) = var X + var x_i − 2·cov(X, x_i).
        let nf = n as f64;
        let mt = totals.iter().sum::<f64>() / nf;
        let item_var: Vec<f64> = cols.iter().map(|c| pop_var(c)).collect();
        let sum_var: f64 = item_var.iter().sum();
        let alpha_if_deleted: Vec<Option<f64>> = cols
            .iter()
            .zip(&item_var)
            .map(|(c, vi)| {
                if k < 3 {
                    return None;
                }
                let mi = c.iter().sum::<f64>() / nf;
                let cov = c
                    .iter()
                    .zip(&totals)
                    .map(|(x, t)| (x - mi) * (t - mt))
                    .sum::<f64>()
                    / nf;
                let vr = vt + vi - 2.0 * cov;
                let km = (k - 1) as f64;
                (vr > 1e-12).then(|| km / (km - 1.0) * (1.0 - (sum_var - vi) / vr))
            })
            .collect();
        let alpha_rise_items = match alpha {
            Some(a) => items
                .iter()
                .zip(&alpha_if_deleted)
                .filter(|(_, ad)| ad.is_some_and(|x| x - a >= 0.01))
                .map(|(i, _)| *i)
                .collect(),
            None => Vec::new(),
        };
        out.push(TestStats {
            n,
            summary: summary(&totals),
            kr20,
            kr21,
            alpha,
            alpha_ci: alpha.and_then(|a| feldt_ci(a, n, k)),
            alpha_if_deleted,
            alpha_rise_items,
            sem: alpha.map(|a| vt.sqrt() * (1.0 - a).max(0.0).sqrt()),
            band: alpha.map(|a| {
                if a >= 0.90 {
                    AlphaBand::HighStakes
                } else if a >= 0.80 {
                    AlphaBand::EndOfCourse
                } else if a >= 0.70 {
                    AlphaBand::LowStakes
                } else {
                    AlphaBand::Below
                }
            }),
            low_confidence: n < 30,
            items,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_bands_groups_and_helpers() {
        assert_eq!(SmallN::of(29), SmallN::Low);
        assert_eq!(SmallN::of(30), SmallN::Limited);
        assert_eq!(SmallN::of(50), SmallN::Moderate);
        assert_eq!(Discrimination::of(-0.01), Discrimination::ProbableFlaw);
        assert_eq!(Discrimination::of(0.25), Discrimination::Acceptable);
        // Ties at the 27 % cut are all included: totals 1,2,2,2,3 with ⌈5·.27⌉ = 2 -> the
        // lower cut is 2, so four examinees are "lower".
        let (up, lo) = upper_lower(&[1.0, 2.0, 2.0, 2.0, 3.0], 0.27);
        assert_eq!(lo.iter().filter(|b| **b).count(), 4);
        assert_eq!(up.iter().filter(|b| **b).count(), 4);
        assert_eq!(
            pearson(&[1.0, 1.0], &[0.0, 1.0]),
            None,
            "zero variance is n/a"
        );
        assert!((pearson(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(csem_lord(3.0, 1), None);
        assert!(sort_key(-1.0) < sort_key(0.0) && sort_key(0.0) < sort_key(0.5));
    }
}
