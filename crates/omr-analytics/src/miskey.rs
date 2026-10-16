//! Ranked mis-key and flawed-item detection (spec §12.2) with the §12.3 small-N rules and
//! bootstrap flag stability. Flags never re-key on their own.

use crate::ctt::{ItemStats, OptionStats, SmallN};
use crate::data::AnalysisData;
use omr_model::{Exception, Scope, Severity};
use omr_scoring::key::letter;
use omr_scoring::stats::SplitMix64;
use serde::{Deserialize, Serialize};

/// Thresholds of the §12.1–§12.2 rules (all configurable).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsConfig {
    /// Rule 1: a distractor with r_d above this.
    pub miskey_distractor_r: f64,
    /// Rule 4: two options with r above this.
    pub two_correct_r: f64,
    /// Rule 5: p ≤ 1/m + this margin.
    pub near_chance_margin: f64,
    /// Rules 5–6: |r_ir| below this.
    pub low_r: f64,
    /// Rule 6: non-functioning distractor below this share.
    pub non_functioning_p: f64,
    /// Rule 6: "little information" above this p…
    pub easy_p: f64,
    /// …with |r_ir| below this.
    pub r_near_zero: f64,
    /// Upper/lower group fraction (Kelley 27 %).
    pub upper_fraction: f64,
    /// Bootstrap resamples for flag stability.
    pub bootstrap_b: usize,
    /// Shared reports show cells with n below this as "< n".
    pub suppress_n: usize,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            miskey_distractor_r: 0.10,
            two_correct_r: 0.15,
            near_chance_margin: 0.05,
            low_r: 0.10,
            non_functioning_p: 0.05,
            easy_p: 0.95,
            r_near_zero: 0.05,
            upper_fraction: 0.27,
            bootstrap_b: 1000,
            suppress_n: 5,
        }
    }
}

/// The §12.2 rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rule {
    /// Rank 1: r_ir(key) < 0 and a distractor has r_d > 0.10 (SCO-001).
    ProbableMiskey,
    /// Rank 1 (multi-select): a keyed option with r_o < 0 while an unkeyed one has
    /// r_o > 0.10 (SCO-001).
    MiskeyedOption,
    /// Rank 2: the upper 27 % prefer a distractor (SCO-002).
    TopGroupPrefers,
    /// Rank 3: p(key) < 1/m and p(d) > p(key) with r_d > r_key (SCO-002).
    DistractorOutperformsKey,
    /// Rank 4: two options each with r > 0.15 (SCO-002).
    TwoCorrect,
    /// Rank 5: p ≤ 1/m + 0.05 and |r_ir| < 0.10 (item-report flag).
    TooHardOrGuessed,
    /// Rank 6: r_ir < 0.10 (INFO).
    LowDiscrimination,
    /// Rank 6: P(d) < 5 % (INFO).
    NonFunctioningDistractor,
    /// Rank 6: p > 0.95 with r_ir ≈ 0 (INFO).
    LittleInformation,
}

impl Rule {
    /// Rank (1 = most severe).
    pub fn rank(self) -> u8 {
        match self {
            Rule::ProbableMiskey | Rule::MiskeyedOption => 1,
            Rule::TopGroupPrefers => 2,
            Rule::DistractorOutperformsKey => 3,
            Rule::TwoCorrect => 4,
            Rule::TooHardOrGuessed => 5,
            _ => 6,
        }
    }

    /// Appendix A code, if the rule raises one.
    pub fn code(self) -> Option<&'static str> {
        match self.rank() {
            1 => Some("SCO-001"),
            2..=4 => Some("SCO-002"),
            _ => None,
        }
    }
}

/// How much a flag can be trusted at this N (§12.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Confidence {
    /// Normal.
    Normal,
    /// Low confidence (30 ≤ N < 50 for rules 3–4; bootstrap stability < 50 %).
    Low,
    /// "Possible — insufficient data" (rules 1–2 at 10 ≤ N < 30).
    InsufficientData,
}

/// One ranked flag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemFlag {
    /// Item index in [`AnalysisData::items`].
    pub item: usize,
    /// Master item index in the score run (the exception's `question` target).
    pub master: usize,
    /// Item label.
    pub label: String,
    /// Rule.
    pub rule: Rule,
    /// The distractor or option concerned.
    pub option: Option<u8>,
    /// Message.
    pub message: String,
    /// Confidence.
    pub confidence: Confidence,
    /// Share of bootstrap resamples where the rule fires (filled by [`flag_stability`]).
    pub stability: Option<f64>,
}

impl ItemFlag {
    /// Rank of the rule.
    pub fn rank(&self) -> u8 {
        self.rule.rank()
    }
}

fn opt(o: Option<u8>) -> String {
    o.map_or("omit".into(), |o| letter(o).to_string())
}

/// Applies the §12.2 rules to computed item statistics, with the §12.3 small-N handling,
/// sorted by rank then item.
pub fn detect(data: &AnalysisData, stats: &[ItemStats], cfg: &AnalyticsConfig) -> Vec<ItemFlag> {
    let mut flags = Vec::new();
    for st in stats {
        let it = &data.items[st.item];
        if st.tier == SmallN::Minimal || it.options == 0 {
            continue; // counts, p and option distribution only
        }
        let mut push = |rule: Rule, option: Option<u8>, message: String| {
            let rank = rule.rank();
            let confidence = match (st.tier, rank) {
                (SmallN::Low, 1 | 2) => Confidence::InsufficientData,
                (SmallN::Low, 3 | 4) => return, // suppressed
                (SmallN::Limited, 3 | 4) => Confidence::Low,
                _ => Confidence::Normal,
            };
            let message = if confidence == Confidence::InsufficientData {
                format!("{message} (possible — insufficient data)")
            } else {
                message
            };
            flags.push(ItemFlag {
                item: st.item,
                master: it.master,
                label: st.label.clone(),
                rule,
                option,
                message,
                confidence,
                stability: None,
            });
        };
        let real: Vec<&OptionStats> = st.options.iter().filter(|o| o.option.is_some()).collect();
        let keyed: Vec<&&OptionStats> = real.iter().filter(|o| o.keyed).collect();
        let distractors: Vec<&&OptionStats> = real.iter().filter(|o| !o.keyed).collect();
        let m = f64::from(it.options);
        if it.multi_select {
            // Multi-select: a keyed option with r_o < 0 while an unkeyed one has r_o > 0.10.
            let bad_key = keyed.iter().find(|o| o.r.is_some_and(|r| r < 0.0));
            let good_d = distractors
                .iter()
                .filter(|o| o.r.is_some_and(|r| r > cfg.miskey_distractor_r))
                .max_by(|a, b| a.r.unwrap_or(0.0).total_cmp(&b.r.unwrap_or(0.0)));
            if let (Some(k), Some(d)) = (bad_key, good_d) {
                push(
                    Rule::MiskeyedOption,
                    d.option,
                    format!(
                        "Probable mis-keyed option: {} (r = {:.2}) keyed, {} (r = {:.2}) not",
                        opt(k.option),
                        k.r.unwrap_or(0.0),
                        opt(d.option),
                        d.r.unwrap_or(0.0)
                    ),
                );
            }
        } else {
            let r_key = st.r_ir;
            // Rule 1.
            if let Some(rk) = r_key
                && rk < 0.0
                && let Some(d) = distractors
                    .iter()
                    .filter(|o| o.r.is_some_and(|r| r > cfg.miskey_distractor_r))
                    .max_by(|a, b| a.r.unwrap_or(0.0).total_cmp(&b.r.unwrap_or(0.0)))
            {
                push(
                    Rule::ProbableMiskey,
                    d.option,
                    format!(
                        "Probable mis-key: {} (r_key = {rk:.2}, r_{} = {:.2})",
                        opt(d.option),
                        opt(d.option),
                        d.r.unwrap_or(0.0)
                    ),
                );
            }
            // Rule 2: the upper group prefers a distractor.
            let key_upper = keyed.iter().map(|o| o.upper).fold(0.0, f64::max);
            if let Some(d) = distractors
                .iter()
                .filter(|o| o.upper > key_upper)
                .max_by(|a, b| a.upper.total_cmp(&b.upper))
            {
                push(
                    Rule::TopGroupPrefers,
                    d.option,
                    format!(
                        "Top group prefers {} ({:.0} % vs key {:.0} %)",
                        opt(d.option),
                        d.upper * 100.0,
                        key_upper * 100.0
                    ),
                );
            }
            // Rule 3: distractor outperforms key.
            let p_key: f64 = keyed.iter().map(|o| o.proportion).sum();
            let r_key_opt = keyed.first().and_then(|o| o.r);
            if p_key < 1.0 / m
                && let Some(d) = distractors.iter().find(|o| {
                    o.proportion > p_key
                        && match (o.r, r_key_opt) {
                            (Some(rd), Some(rk)) => rd > rk,
                            (Some(_), None) => true,
                            _ => false,
                        }
                })
            {
                push(
                    Rule::DistractorOutperformsKey,
                    d.option,
                    format!(
                        "Distractor outperforms key: {} (p = {:.2}, key p = {p_key:.2})",
                        opt(d.option),
                        d.proportion
                    ),
                );
            }
            // Rule 4: two options each with r > 0.15.
            let strong: Vec<&&OptionStats> = real
                .iter()
                .filter(|o| o.r.is_some_and(|r| r > cfg.two_correct_r))
                .collect();
            if strong.len() >= 2 {
                let other = strong.iter().find(|o| !o.keyed).or(strong.get(1));
                push(
                    Rule::TwoCorrect,
                    other.and_then(|o| o.option),
                    format!(
                        "Possibly two correct answers: {}",
                        strong
                            .iter()
                            .map(|o| opt(o.option))
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ),
                );
            }
        }
        // Rule 5.
        if let (Some(p), Some(r)) = (st.p, st.r_ir)
            && p <= 1.0 / m + cfg.near_chance_margin
            && r.abs() < cfg.low_r
        {
            push(
                Rule::TooHardOrGuessed,
                None,
                format!("Too hard or guessed (p = {p:.2}, r = {r:.2})"),
            );
        }
        // Rule 6.
        if let Some(r) = st.r_ir
            && r < cfg.low_r
        {
            push(
                Rule::LowDiscrimination,
                None,
                format!("Low discrimination (r_ir = {r:.2})"),
            );
        }
        for d in distractors
            .iter()
            .filter(|o| o.proportion < cfg.non_functioning_p)
        {
            push(
                Rule::NonFunctioningDistractor,
                d.option,
                format!(
                    "Non-functioning distractor {} (P = {:.1} %)",
                    opt(d.option),
                    d.proportion * 100.0
                ),
            );
        }
        if let (Some(p), Some(r)) = (st.p, st.r_ir)
            && p > cfg.easy_p
            && r.abs() < cfg.r_near_zero
        {
            push(
                Rule::LittleInformation,
                None,
                format!("Little information (p = {p:.2})"),
            );
        }
    }
    flags.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then(a.item.cmp(&b.item))
            .then(a.rule.cmp(&b.rule))
    });
    flags
}

/// Bootstrap flag stability (§12.2, §12.3): the share of `b` resamples of examinees in which
/// each flag's rule fires again for the same item and option; seeded from the run ID.
/// Flags below 50 % become low-confidence. Re-sorts by rank, then stability (descending).
/// Cost ≈ b × N × Σm cell operations: the app runs it as a background job for N < 1,000.
pub fn flag_stability(
    data: &AnalysisData,
    flags: &mut [ItemFlag],
    cfg: &AnalyticsConfig,
    b: usize,
) {
    if flags.is_empty() || data.n == 0 || b == 0 {
        return;
    }
    let mut rng = SplitMix64::from_run(&data.run_id, "flag_stability");
    let mut hits = vec![0usize; flags.len()];
    for _ in 0..b {
        let rows: Vec<usize> = (0..data.n)
            .map(|_| rng.below(data.n as u64) as usize)
            .collect();
        let sample = resample(data, &rows);
        let st = crate::ctt::item_stats(&sample, cfg.upper_fraction);
        let again = detect(&sample, &st, cfg);
        for (k, f) in flags.iter().enumerate() {
            if again
                .iter()
                .any(|g| g.item == f.item && g.rule == f.rule && g.option == f.option)
            {
                hits[k] += 1;
            }
        }
    }
    for (f, h) in flags.iter_mut().zip(hits) {
        let s = h as f64 / b as f64;
        f.stability = Some(s);
        if s < 0.5 && f.confidence == Confidence::Normal {
            f.confidence = Confidence::Low;
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
    });
}

fn resample(data: &AnalysisData, rows: &[usize]) -> AnalysisData {
    let n = rows.len();
    let k = data.items.len();
    let pick = |v: &Vec<f64>| -> Vec<f64> {
        (0..k)
            .flat_map(|i| rows.iter().map(move |&e| (i, e)))
            .map(|(i, e)| v[i * data.n + e])
            .collect()
    };
    AnalysisData {
        items: data.items.clone(),
        n,
        u: pick(&data.u),
        x: pick(&data.x),
        choice: (0..k)
            .flat_map(|i| rows.iter().map(move |&e| data.choice[i * data.n + e]))
            .collect(),
        status: (0..k)
            .flat_map(|i| rows.iter().map(move |&e| data.status[i * data.n + e]))
            .collect(),
        penalties: data.penalties,
        run_id: data.run_id.clone(),
    }
}

/// Exceptions for the flags (SCO-001 ERROR, SCO-002 WARN; Exam scope), one per flag that
/// carries a code, targeted at the master item and master option.
pub fn flag_exceptions(flags: &[ItemFlag]) -> Vec<Exception> {
    flags
        .iter()
        .filter_map(|f| {
            let code = f.rule.code()?;
            let sev = if code == "SCO-001" {
                Severity::Error
            } else {
                Severity::Warn
            };
            Some(
                Exception::must(code, sev, Scope::Exam)
                    .with_target(omr_model::exceptions::Target {
                        question: u16::try_from(f.master).ok(),
                        bubble: f.option,
                        ..Default::default()
                    })
                    .with_detail(format!("{}: {}", f.label, f.message)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ranks_and_codes() {
        assert_eq!(Rule::ProbableMiskey.code(), Some("SCO-001"));
        assert_eq!(Rule::MiskeyedOption.rank(), 1);
        assert_eq!(Rule::TwoCorrect.code(), Some("SCO-002"));
        assert_eq!(Rule::TooHardOrGuessed.code(), None);
        assert_eq!(Rule::NonFunctioningDistractor.rank(), 6);
        assert_eq!(opt(None), "omit");
        assert_eq!(opt(Some(2)), "C");
    }
}
