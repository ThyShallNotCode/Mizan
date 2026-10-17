//! Flag stability (spec §12.2 "Flag stability", §12.3 "Bootstrap"): how far a mis-key or
//! flawed-item flag (rules 1–4) survives sampling noise. A **separate job**, never on the
//! synchronous ≤ 50 ms regrade path ([`crate::analyze`] leaves it out); the app runs
//! [`stability_job`] in the background after each key change and merges the result with
//! [`apply_stability`].
//!
//! - **N < 1,000**: the share of 1,000 non-parametric bootstrap resamples of examinees in which
//!   the same rule fires again for the same item and option. Resamples are drawn from a
//!   SplitMix64 seeded from the **content** of the analysis input ([`content_seed`]: every
//!   item's options, maximum and keyed set, every cell), never from the score run's version
//!   history, so the job is pure and deterministic: the same responses scored with the same key
//!   give the same shares on every machine, whichever key versions led there.
//! - **N ≥ 1,000**: no bootstrap; the rule is evaluated at both ends of the 95 % Fisher-z CI
//!   of every correlation it uses.
//! - The CI ends are reported at every N (Fisher-z, as the synchronous item report shows them):
//!   a flag whose share is not above 50 % with Monte Carlo confidence (the share's 95 %
//!   binomial interval over the B resamples must lie above 50 %: at B = 1,000 a share of 52 %
//!   is ±3 points of sampling noise), or whose rule fails at one end of the CI, is **low
//!   confidence *(proposed)***. (A single high scorer choosing a distractor on an item nearly
//!   everyone answers correctly gives r_ir < 0 with a CI far across 0; a genuine mis-key keeps
//!   its CI below 0 and fires in almost every resample.)
//! - With the bootstrap, the same resamples give every item's **percentile 95 % CI of r_ir**
//!   (§12.3: "bootstrap CIs on r_ir always shown" below N = 100), exported beside the
//!   synchronous Fisher-z CI.
//!
//! Only the flagged items get their full statistics recomputed per resample (each examinee's
//! number-right total does not change when examinees are resampled); every other item only its
//! r_ir, so the job costs ≈ B × (N log N + N × items).

use crate::AnalyticsReport;
use crate::ctt::{ItemStats, OptionStats, RowView, item_stat, number_right, r_ir_of, upper_lower};
use crate::data::{AnalysisData, CellStatus};
use crate::miskey::{AnalyticsConfig, Confidence, ItemFlag, Rule, detect_item, flag_exceptions};
use omr_scoring::stats::{SplitMix64, fisher_z_ci, percentile_sorted};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};

/// Below this many examinees the bootstrap runs; at or above it the Fisher-z rule decides.
pub const BOOTSTRAP_MAX_N: usize = 1_000;

/// The statistic the PRNG seed is derived from (with [`content_seed`]).
pub const SEED_LABEL: &str = "flag_stability";

/// A bootstrap percentile CI of r_ir is given when r_ir is defined in at least this share of
/// the resamples (it is undefined when a resample has no variance on the item, e.g. when the
/// one examinee who missed an easy item is not drawn).
pub const CI_MIN_DEFINED: f64 = 0.9;

/// The content digest the bootstrap draws are seeded from (§12 "Recompute": the seed follows
/// the score run's content, as the spec's content-addressed run ID does): SHA-384 (hex) over
/// N, every item's printed options, maximum, keyed set and kind, and every cell (status, u, x
/// and the chosen options), in data order. Labels, master indices and the key's version
/// number and history do not enter it, so the same responses scored with the same key and
/// policy give the same shares whichever key versions led there.
pub fn content_seed(data: &AnalysisData) -> String {
    let mut h = Sha384::new();
    h.update(b"omr-analytics/flag-stability/1");
    h.update((data.n as u64).to_le_bytes());
    h.update((data.items.len() as u64).to_le_bytes());
    for it in &data.items {
        h.update([it.options, u8::from(it.multi_select), u8::from(it.choice)]);
        h.update(it.max.to_bits().to_le_bytes());
        h.update(it.keyed.to_le_bytes());
    }
    for (k, st) in data.status.iter().enumerate() {
        let s = match st {
            CellStatus::Reached => 0u8,
            CellStatus::NotReached => 1,
            CellStatus::NotPresented => 2,
        };
        h.update([s]);
        h.update(data.u[k].to_bits().to_le_bytes());
        h.update(data.x[k].to_bits().to_le_bytes());
        h.update(data.choice[k].to_le_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The 95 % Monte Carlo interval of a share estimated from `b` resamples (normal approximation
/// to the binomial, clamped to [0, 1]).
pub fn mc_interval(share: f64, b: usize) -> (f64, f64) {
    if b == 0 {
        return (0.0, 1.0);
    }
    let half = 1.96 * (share * (1.0 - share) / b as f64).sqrt();
    ((share - half).max(0.0), (share + half).min(1.0))
}

/// The bootstrap percentile 95 % CI of one item's r_ir (§12.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemCi {
    /// Item index in the analysis data.
    pub item: usize,
    /// Master item index.
    pub master: usize,
    /// Item label.
    pub label: String,
    /// 2.5th and 97.5th percentiles of r_ir over the resamples where it is defined; `None`
    /// when it is defined in fewer than [`CI_MIN_DEFINED`] of them.
    pub ci: Option<(f64, f64)>,
    /// Resamples where r_ir was defined.
    pub defined: usize,
}

/// How stability was judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StabilityMethod {
    /// Bootstrap share of resamples (N < 1,000), plus the CI ends.
    Bootstrap,
    /// Both ends of the 95 % Fisher-z CI (N ≥ 1,000).
    FisherZ,
}

/// The stability of one rank 1–4 flag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlagStability {
    /// Item index in the analysis data.
    pub item: usize,
    /// Master item index.
    pub master: usize,
    /// Item label.
    pub label: String,
    /// Rule.
    pub rule: Rule,
    /// Option concerned.
    pub option: Option<u8>,
    /// Share of bootstrap resamples where the rule fires again (N < 1,000).
    pub share: Option<f64>,
    /// The share's 95 % Monte Carlo interval over the resamples drawn.
    #[serde(default)]
    pub share_mc: Option<(f64, f64)>,
    /// Whether the rule still holds at the (lower, upper) end of the 95 % CI of the statistic
    /// that decides it (Fisher-z: r_ir(key) for rules 1 and 3, option r for rule 4 and the
    /// multi-select rule; rule 2 compares upper-group proportions, with a normal CI of their
    /// difference). `None` when a statistic is undefined.
    pub ci_ends: Option<(bool, bool)>,
    /// Low confidence *(proposed)*: the share's 95 % Monte Carlo interval does not lie above
    /// 50 %, or the rule fails at one CI end.
    pub low_confidence: bool,
}

impl FlagStability {
    /// A short human-readable account, e.g. "fires in 31 % of 1,000 bootstrap resamples
    /// (Monte Carlo 95 % interval 28–34 %); fails at the upper end of its 95 % CI".
    pub fn describe(&self, resamples: usize) -> String {
        let mut parts = Vec::new();
        if let Some(s) = self.share {
            let mc = self
                .share_mc
                .map(|(lo, hi)| {
                    format!(
                        " (Monte Carlo 95 % interval {:.0}–{:.0} %)",
                        lo * 100.0,
                        hi * 100.0
                    )
                })
                .unwrap_or_default();
            parts.push(format!(
                "fires in {:.0} % of {} bootstrap resamples{mc}",
                s * 100.0,
                group_thousands(resamples)
            ));
        }
        match self.ci_ends {
            Some((true, true)) => parts.push("holds at both ends of its 95 % CI".into()),
            Some((false, true)) => parts.push("fails at the lower end of its 95 % CI".into()),
            Some((true, false)) => parts.push("fails at the upper end of its 95 % CI".into()),
            Some((false, false)) => parts.push("fails at both ends of its 95 % CI".into()),
            None => {}
        }
        parts.join("; ")
    }
}

fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The output of the stability job for one score run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StabilityReport {
    /// Method by N.
    pub method: StabilityMethod,
    /// Examinees.
    pub n: usize,
    /// Bootstrap resamples (0 for the Fisher-z method).
    pub resamples: usize,
    /// The score run the job was computed for (the app keeps one report per score run).
    pub run_id: String,
    /// The content digest the resamples are seeded from (with [`SEED_LABEL`];
    /// [`content_seed`]); empty for the Fisher-z method, which draws nothing.
    #[serde(default)]
    pub seed: String,
    /// One entry per rank 1–4 flag, in the order of the flags given.
    pub flags: Vec<FlagStability>,
    /// Bootstrap percentile 95 % CI of r_ir for every item with N ≥ 10 (bootstrap method only).
    #[serde(default)]
    pub item_ci: Vec<ItemCi>,
}

impl StabilityReport {
    /// The bootstrap percentile CI of r_ir of data item `item`, when computed.
    pub fn r_ir_ci(&self, item: usize) -> Option<(f64, f64)> {
        self.item_ci
            .iter()
            .find(|c| c.item == item)
            .and_then(|c| c.ci)
    }
}

/// The stability job (§12.2, §12.3): bootstrap with `cfg.bootstrap_b` resamples when
/// N < 1,000 (flag shares and every item's percentile CI of r_ir), else the Fisher-z rule; CI
/// ends at every N. Pure and deterministic (seeded from the data's content); run it off the
/// synchronous path.
pub fn stability_job(
    data: &AnalysisData,
    flags: &[ItemFlag],
    cfg: &AnalyticsConfig,
) -> StabilityReport {
    let b = (data.n < BOOTSTRAP_MAX_N).then_some(cfg.bootstrap_b);
    stability_with(data, flags, cfg, b)
}

/// The job with an explicit bootstrap size (`None`: Fisher-z only).
pub(crate) fn stability_with(
    data: &AnalysisData,
    flags: &[ItemFlag],
    cfg: &AnalyticsConfig,
    b: Option<usize>,
) -> StabilityReport {
    let targets: Vec<&ItemFlag> = flags.iter().filter(|f| f.rank() <= 4).collect();
    let mut items: Vec<usize> = targets.iter().map(|f| f.item).collect();
    items.sort_unstable();
    items.dedup();
    // Full-data statistics of the flagged items (for the CI ends).
    let (nr, nr_max) = number_right(data);
    let (upper, lower) = upper_lower(&nr, cfg.upper_fraction);
    let rows: Vec<usize> = (0..data.n).collect();
    let view = RowView {
        rows: &rows,
        nr: &nr,
        nr_max: &nr_max,
        upper: &upper,
        lower: &lower,
    };
    let mut keys = Vec::new();
    let full: Vec<(usize, ItemStats)> = items
        .iter()
        .map(|&i| (i, item_stat(data, i, &view, &mut keys)))
        .collect();
    let stats_of = |i: usize| full.iter().find(|(j, _)| *j == i).map(|(_, s)| s);
    let b = b.filter(|&b| b > 0 && data.n > 0);
    let seed = b.map(|_| content_seed(data)).unwrap_or_default();
    let boot = b.map(|b| bootstrap(data, &targets, &items, cfg, b, &nr, &nr_max, &seed));
    let out = targets
        .iter()
        .enumerate()
        .map(|(t, f)| {
            let ci_ends = stats_of(f.item).and_then(|st| ci_ends(f, st, data, cfg));
            let share = boot.as_ref().map(|(s, _)| s[t]);
            let share_mc = share.zip(b).map(|(s, b)| mc_interval(s, b));
            let low_confidence = is_low_confidence(share_mc, ci_ends);
            FlagStability {
                item: f.item,
                master: f.master,
                label: f.label.clone(),
                rule: f.rule,
                option: f.option,
                share,
                share_mc,
                ci_ends,
                low_confidence,
            }
        })
        .collect();
    StabilityReport {
        method: if b.is_some() {
            StabilityMethod::Bootstrap
        } else {
            StabilityMethod::FisherZ
        },
        n: data.n,
        resamples: b.unwrap_or(0),
        run_id: data.run_id.clone(),
        seed,
        flags: out,
        item_ci: boot.map(|(_, ci)| ci).unwrap_or_default(),
    }
}

/// The §12.2 label: low confidence *(proposed)* unless the share is above 50 % beyond Monte
/// Carlo noise (its 95 % interval lies above 50 %) and the rule holds at both CI ends.
fn is_low_confidence(share_mc: Option<(f64, f64)>, ci_ends: Option<(bool, bool)>) -> bool {
    share_mc.is_some_and(|(lo, _)| lo <= 0.5) || ci_ends.is_some_and(|(lo, hi)| !(lo && hi))
}

/// The bootstrap: per target flag, the share of `b` resamples in which its rule fires again;
/// per item with N ≥ 10, the percentile 95 % CI of r_ir over the same resamples.
#[allow(clippy::too_many_arguments)]
fn bootstrap(
    data: &AnalysisData,
    targets: &[&ItemFlag],
    items: &[usize],
    cfg: &AnalyticsConfig,
    b: usize,
    nr: &[f64],
    nr_max: &[f64],
    seed: &str,
) -> (Vec<f64>, Vec<ItemCi>) {
    let n = data.n;
    let mut rng = SplitMix64::from_run(seed, SEED_LABEL);
    // Every item with a defined r_ir on the full data gets a CI.
    let identity: Vec<usize> = (0..n).collect();
    let (up0, lo0) = upper_lower(nr, cfg.upper_fraction);
    let full = RowView {
        rows: &identity,
        nr,
        nr_max,
        upper: &up0,
        lower: &lo0,
    };
    let (mut ub, mut rb) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let ci_items: Vec<usize> = (0..data.items.len())
        .filter(|&i| r_ir_of(data, i, &full, &mut ub, &mut rb).is_some())
        .collect();
    let mut draws: Vec<Vec<f64>> = vec![Vec::with_capacity(b); ci_items.len()];
    let mut hits = vec![0usize; targets.len()];
    let mut rows = vec![0usize; n];
    let mut rnr = vec![0.0; n];
    let mut rmax = vec![0.0; n];
    let mut keys = Vec::with_capacity(n);
    let mut found = Vec::new();
    for _ in 0..b {
        for k in 0..n {
            let e = rng.below(n as u64) as usize;
            rows[k] = e;
            rnr[k] = nr[e];
            rmax[k] = nr_max[e];
        }
        // Upper/lower groups by the resample's own number-right totals.
        let (up, lo) = upper_lower(&rnr, cfg.upper_fraction);
        let view = RowView {
            rows: &rows,
            nr: &rnr,
            nr_max: &rmax,
            upper: &up,
            lower: &lo,
        };
        for &i in items {
            let st = item_stat(data, i, &view, &mut keys);
            found.clear();
            detect_item(data, &st, cfg, &mut found);
            for (t, f) in targets.iter().enumerate() {
                if f.item == i
                    && found
                        .iter()
                        .any(|g| g.rule == f.rule && g.option == f.option)
                {
                    hits[t] += 1;
                }
            }
        }
        for (c, &i) in ci_items.iter().enumerate() {
            if let Some(r) = r_ir_of(data, i, &view, &mut ub, &mut rb) {
                draws[c].push(r);
            }
        }
    }
    let shares = hits.iter().map(|&h| h as f64 / b as f64).collect();
    let cis = ci_items
        .iter()
        .zip(draws.iter_mut())
        .map(|(&i, d)| {
            d.sort_by(f64::total_cmp);
            let ok = d.len() as f64 >= CI_MIN_DEFINED * b as f64;
            ItemCi {
                item: i,
                master: data.items[i].master,
                label: data.items[i].label.clone(),
                ci: ok.then(|| (percentile_sorted(d, 0.025), percentile_sorted(d, 0.975))),
                defined: d.len(),
            }
        })
        .collect();
    (shares, cis)
}

fn option_stats(st: &ItemStats, o: Option<u8>) -> Option<&OptionStats> {
    st.options
        .iter()
        .find(|s| s.option.is_some() && s.option == o)
}

/// Whether the flag's rule holds at the lower and upper ends of the 95 % CI of the statistic
/// that decides it: r_ir(key) for rules 1 and 3 (the Fisher-z CI the item report shows,
/// §12.1), the keyed option's r for the multi-select rule, both options' r for rule 4, and the
/// upper-group proportion difference for rule 2 (normal CI; it has no correlation).
/// Thresholds on other statistics keep their point estimates.
fn ci_ends(
    f: &ItemFlag,
    st: &ItemStats,
    data: &AnalysisData,
    cfg: &AnalyticsConfig,
) -> Option<(bool, bool)> {
    let n = st.n as u64;
    let ci = |r: f64| fisher_z_ci(r, n);
    let _ = data;
    match f.rule {
        Rule::ProbableMiskey => {
            // r_ir(key) < 0 at both ends (the distractor's r_d > 0.10 is a point filter).
            let (kl, kh) = st.r_ir_ci.or_else(|| ci(st.r_ir?))?;
            Some((kl < 0.0, kh < 0.0))
        }
        Rule::MiskeyedOption => {
            let rk = st
                .options
                .iter()
                .filter(|o| o.keyed)
                .filter_map(|o| o.r)
                .find(|&r| r < 0.0)?;
            let (kl, kh) = ci(rk)?;
            Some((kl < 0.0, kh < 0.0))
        }
        Rule::TopGroupPrefers => {
            // Normal CI of p_U(d) − p_U(key) over the upper group (multinomial covariance).
            let pd = option_stats(st, f.option)?.upper;
            let pk = st
                .options
                .iter()
                .filter(|o| o.keyed)
                .map(|o| o.upper)
                .fold(0.0, f64::max);
            let n_u = (st.n as f64 * cfg.upper_fraction).ceil().max(1.0);
            let diff = pd - pk;
            let se = ((pd + pk - diff * diff).max(0.0) / n_u).sqrt();
            Some((diff - 1.96 * se > 0.0, diff + 1.96 * se > 0.0))
        }
        Rule::DistractorOutperformsKey => {
            // r_d > r_key at both ends of r_key's CI.
            let rd = option_stats(st, f.option)?.r?;
            match st.options.iter().filter(|o| o.keyed).find_map(|o| o.r) {
                Some(rk) => {
                    let (kl, kh) = ci(rk)?;
                    Some((rd > kl, rd > kh))
                }
                None => Some((true, true)),
            }
        }
        Rule::TwoCorrect => {
            let mut rs: Vec<f64> = st
                .options
                .iter()
                .filter(|o| o.option.is_some())
                .filter_map(|o| o.r)
                .filter(|&r| r > cfg.two_correct_r)
                .collect();
            rs.sort_by(|a, b| b.total_cmp(a));
            let (a, b2) = (*rs.first()?, *rs.get(1)?);
            let ((al, ah), (bl, bh)) = (ci(a)?, ci(b2)?);
            let t = cfg.two_correct_r;
            Some((al > t && bl > t, ah > t && bh > t))
        }
        _ => None,
    }
}

/// Merges a stability report into flags: shares into `stability`, low-confidence flags to
/// [`Confidence::Low`] with "low confidence (proposed): …" appended to the message, then
/// re-sorts by rank, then stability (descending), then item.
pub fn apply_to_flags(flags: &mut [ItemFlag], report: &StabilityReport) {
    for f in flags.iter_mut() {
        let Some(s) = report
            .flags
            .iter()
            .find(|s| s.item == f.item && s.rule == f.rule && s.option == f.option)
        else {
            continue;
        };
        f.stability = s.share;
        if s.low_confidence {
            if f.confidence == Confidence::Normal {
                f.confidence = Confidence::Low;
            }
            if !f.message.contains("low confidence (proposed)") {
                f.message = format!(
                    "{} — low confidence (proposed): {}",
                    f.message,
                    s.describe(report.resamples)
                );
            }
        }
    }
    flags.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then(
                b.stability
                    .unwrap_or(0.0)
                    .total_cmp(&a.stability.unwrap_or(0.0)),
            )
            .then(a.item.cmp(&b.item))
            .then(a.rule.cmp(&b.rule))
    });
}

/// Merges the job's result into a synchronous report: the flags (see [`apply_to_flags`]), the
/// SCO-001/SCO-002 exceptions rebuilt from them (their details now carry the low-confidence
/// label), and `report.stability` for exports and view-models.
pub fn apply_stability(report: &mut AnalyticsReport, stability: StabilityReport) {
    apply_to_flags(&mut report.flags, &stability);
    let mut exceptions = flag_exceptions(&report.flags);
    exceptions.extend(
        report
            .exceptions
            .iter()
            .filter(|e| e.code != "SCO-001" && e.code != "SCO-002")
            .cloned(),
    );
    report.exceptions = exceptions;
    report.stability = Some(stability);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_and_descriptions() {
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(999), "999");
        let s = FlagStability {
            item: 0,
            master: 0,
            label: "Q1".into(),
            rule: Rule::ProbableMiskey,
            option: Some(2),
            share: Some(0.31),
            share_mc: None,
            ci_ends: Some((true, false)),
            low_confidence: true,
        };
        assert_eq!(
            s.describe(1000),
            "fires in 31 % of 1,000 bootstrap resamples; fails at the upper end of its 95 % CI"
        );
        let s = FlagStability {
            share: Some(0.52),
            share_mc: Some(mc_interval(0.52, 1000)),
            ci_ends: Some((true, true)),
            ..s
        };
        assert_eq!(
            s.describe(1000),
            "fires in 52 % of 1,000 bootstrap resamples (Monte Carlo 95 % interval 49–55 %); \
             holds at both ends of its 95 % CI"
        );
    }

    #[test]
    fn a_share_near_half_is_low_confidence_until_monte_carlo_noise_is_excluded() {
        // C8: at B = 1,000 a share of 52 % is 52 ± 3 points: its label would be decided by
        // which resamples were drawn, so it is low confidence; 56 % is clear of 50 %.
        let both = Some((true, true));
        assert!(is_low_confidence(Some(mc_interval(0.52, 1000)), both));
        assert!(is_low_confidence(Some(mc_interval(0.50, 1000)), both));
        assert!(!is_low_confidence(Some(mc_interval(0.56, 1000)), both));
        assert!(!is_low_confidence(Some(mc_interval(1.0, 1000)), both));
        // More resamples narrow the interval: 52 % of 20,000 is clear of 50 %.
        assert!(!is_low_confidence(Some(mc_interval(0.52, 20_000)), both));
        // A CI end that fails keeps it low whatever the share.
        assert!(is_low_confidence(
            Some(mc_interval(1.0, 1000)),
            Some((true, false))
        ));
        // Fisher-z (N ≥ 1,000): no share, the CI ends decide.
        assert!(!is_low_confidence(None, both));
        let (lo, hi) = mc_interval(0.52, 1000);
        assert!((lo - 0.489).abs() < 1e-3 && (hi - 0.551).abs() < 1e-3);
    }
}
