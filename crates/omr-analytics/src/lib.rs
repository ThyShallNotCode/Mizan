//! `omr-analytics`: item and test analytics (spec §12).
//!
//! Pure functions over one score run's L1 choices and uncurved L3 item scores:
//! - [`ctt`]: classical test theory (§12.1): difficulty, upper–lower D, point-biserial,
//!   corrected item–rest r_ir with Fisher-z CI, option r and distractor trace lines, KR-20/21,
//!   Cronbach α with α-if-deleted and the Feldt CI, SEM and Lord's CSEM;
//! - [`miskey`]: the ranked mis-key and flawed-item rules (§12.2) with bootstrap stability;
//! - small-N tiers and suppression (§12.3).
//!
//! Advisory only: nothing here changes a score. Statistics are `f64` with population
//! variance (§10.2).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ctt;
pub mod data;
pub mod miskey;

pub use ctt::{
    AlphaBand, DifficultyFlag, Discrimination, ItemStats, OptionStats, SmallN, Summary, TestStats,
    cronbach_alpha, csem_lord, feldt_ci, item_stats, pearson, test_stats,
};
pub use data::{AnalysisData, CellStatus, ItemInfo};
pub use miskey::{
    AnalyticsConfig, Confidence, ItemFlag, Rule, detect, flag_exceptions, flag_stability,
};

use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};

/// Everything the synchronous (≤ 50 ms) analytics path produces for one score run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsReport {
    /// Per-item statistics.
    pub items: Vec<ItemStats>,
    /// Per item-set class test statistics.
    pub tests: Vec<TestStats>,
    /// Ranked flags (stability filled only by [`flag_stability`]).
    pub flags: Vec<ItemFlag>,
    /// SCO-001, SCO-002, SCO-018 and SCO-023 findings.
    pub exceptions: Vec<Exception>,
}

/// Runs CTT, the mis-key rules and the small-N checks (§12.1–§12.3).
pub fn analyze(data: &AnalysisData, cfg: &AnalyticsConfig) -> AnalyticsReport {
    let items = item_stats(data, cfg.upper_fraction);
    let tests = test_stats(data);
    let flags = detect(data, &items, cfg);
    let mut exceptions = flag_exceptions(&flags);
    for t in &tests {
        if let Some(a) = t.alpha
            && a < 0.5
        {
            let hint = if a < 0.0 {
                "negative α: mis-keyed, multidimensional or mis-scanned? check person fit"
            } else {
                "α < 0.5: check for a mis-key, multidimensionality or a mis-scanned batch"
            };
            exceptions.push(
                Exception::must("SCO-018", Severity::Warn, Scope::Exam).with_detail(format!(
                    "α = {a:.3} over {} items, N = {}: {hint}",
                    t.items.len(),
                    t.n
                )),
            );
        }
    }
    let small: Vec<&ItemStats> = items.iter().filter(|s| s.n < 30).collect();
    if !small.is_empty() {
        exceptions.push(
            Exception::must("SCO-023", Severity::Info, Scope::Exam).with_detail(format!(
                "{} item(s) with N < 30 examinees reaching them: low-confidence statistics{}",
                small.len(),
                if small.iter().any(|s| s.n < 10) {
                    "; N < 10: counts, p and option distribution only"
                } else {
                    ""
                }
            )),
        );
    }
    AnalyticsReport {
        items,
        tests,
        flags,
        exceptions,
    }
}

/// A count for reports shared beyond the instructor: "< n" below the threshold (§12.3).
pub fn suppressed(count: usize, suppress_n: usize) -> String {
    if count < suppress_n {
        format!("< {suppress_n}")
    } else {
        count.to_string()
    }
}
