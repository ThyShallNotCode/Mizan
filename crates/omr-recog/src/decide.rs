//! Group decision logic (brief "Decide per question"; §8.8 adapted to the classical bitonal
//! slice, §8.7 step 7 hysteresis). One rule serves answer questions, ID-grid columns and the
//! version / exam / gender choices.
//!
//! Inputs are integer deltas δ = inner − β_p in 1/1024 units and the page mark level (the median
//! top-1 δ over questions with top-1 ≥ 0.25, clamped to [0.30, 1.0]); n = δ / level.
//!
//! | Outcome | Rule |
//! |---|---|
//! | Blank | top-1 δ < 0.08 |
//! | Multi | ≥ 2 options with n ≥ 0.80 and δ ≥ 0.30 (MRK-002) |
//! | Single | exactly one option with n ≥ 0.55 and δ ≥ 0.20, and the erasure rule: ρ = top2/top1 ≤ 0.5 and top1 − top2 ≥ 0.20; a runner-up above 0.10 logs MRK-003 `resolved_erasure` (INFO) |
//! | Review | anything else: MRK-001 (`weak_mark`, `hysteresis`), MRK-002 (two marks, `uncertain_set`), MRK-003 (unresolved erasure) |
//!
//! Every comparison is on integers. A decision is auto-accepted only if each variable clears
//! its threshold by the hysteresis band h; otherwise the same response is proposed for review.

use crate::config::{DecisionConfig, MultiReview};
use omr_model::LogitMilli;
use serde::{Deserialize, Serialize};

/// The response of a group (decided or proposed).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GroupKind {
    /// Nothing marked.
    Blank,
    /// One option (0-based index within the group).
    Single(u8),
    /// Several options, ascending.
    Multi(Vec<u8>),
}

/// Why a group goes to review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewReason {
    /// A lone mark below the selection thresholds (MRK-001 `weak_mark`).
    WeakMark,
    /// A decision variable within h of its threshold (MRK-001 `hysteresis`).
    Hysteresis,
    /// Two or more marks that are neither a confident multi nor a resolvable erasure (MRK-002).
    TwoMarks,
    /// A multi with a further option near the selection threshold (MRK-002 `uncertain_set`).
    UncertainSet,
    /// A confident multi routed to review by the `multi_review` policy (MRK-002).
    MultiPolicy,
    /// A residue the erasure rule cannot resolve, or the rule is review-only (MRK-003).
    UnresolvedErasure,
}

impl ReviewReason {
    /// `(code, sub_reason)` for the ERROR exception this reason raises.
    pub fn code(self) -> (&'static str, Option<&'static str>) {
        match self {
            ReviewReason::WeakMark => ("MRK-001", Some("weak_mark")),
            ReviewReason::Hysteresis => ("MRK-001", Some("hysteresis")),
            ReviewReason::TwoMarks => ("MRK-002", None),
            ReviewReason::UncertainSet => ("MRK-002", Some("uncertain_set")),
            ReviewReason::MultiPolicy => ("MRK-002", None),
            ReviewReason::UnresolvedErasure => ("MRK-003", None),
        }
    }
}

/// The decision for one group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDecision {
    /// Decided (accepted) or proposed (review) response.
    pub kind: GroupKind,
    /// True if auto-accepted.
    pub accepted: bool,
    /// Why not, when not accepted.
    pub review: Option<ReviewReason>,
    /// An accepted single whose runner-up was a residue (MRK-003 `resolved_erasure`, INFO).
    pub resolved_erasure: bool,
    /// Smallest slack (1/1024) between an accepting variable and its threshold; 0 for review.
    pub margin: i32,
    /// The selection threshold θ_sel = max(0.20, 0.55 · level) used.
    pub theta_sel: i32,
}

fn ceil_div(a: i64, b: i64) -> i64 {
    (a + b - 1).div_euclid(b)
}

/// The page mark level from the top-1 deltas of the page's questions.
pub fn page_level(top1: impl IntoIterator<Item = i32>, cfg: &DecisionConfig) -> i32 {
    let mut clear: Vec<i32> = top1
        .into_iter()
        .filter(|&t| t >= i32::from(cfg.level_clear_min))
        .collect();
    let lvl = if clear.is_empty() {
        i32::from(cfg.level_max)
    } else {
        clear.sort_unstable();
        let n = clear.len();
        if n % 2 == 1 {
            clear[n / 2]
        } else {
            (clear[n / 2 - 1] + clear[n / 2] + 1).div_euclid(2)
        }
    };
    lvl.clamp(i32::from(cfg.level_min), i32::from(cfg.level_max))
}

fn review(kind: GroupKind, why: ReviewReason, theta_sel: i32) -> GroupDecision {
    GroupDecision {
        kind,
        accepted: false,
        review: Some(why),
        resolved_erasure: false,
        margin: 0,
        theta_sel,
    }
}

/// Decides one group of deltas `d` (1/1024) at page level `level` (1/1024).
pub fn decide_group(
    d: &[i32],
    level: i32,
    cfg: &DecisionConfig,
    high_stakes: bool,
) -> GroupDecision {
    let k = d.len();
    let lv = i64::from(level.max(1));
    let theta_sel = i64::from(cfg.select_abs)
        .max(ceil_div(i64::from(cfg.select_rel_permille) * lv, 1000)) as i32;
    let theta_strong = i64::from(cfg.strong_abs)
        .max(ceil_div(i64::from(cfg.strong_rel_permille) * lv, 1000)) as i32;
    let h = i32::from(cfg.hysteresis);
    let blank_max = i32::from(cfg.blank_max);
    if k == 0 {
        return GroupDecision {
            kind: GroupKind::Blank,
            accepted: true,
            review: None,
            resolved_erasure: false,
            margin: blank_max,
            theta_sel,
        };
    }
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|&a, &b| d[b].cmp(&d[a]).then(a.cmp(&b)));
    let t1 = d[order[0]];
    let t2 = if k >= 2 { d[order[1]] } else { 0 };
    let top = order[0] as u8;

    // Blank.
    if t1 < blank_max {
        if t1 < blank_max - h {
            return GroupDecision {
                kind: GroupKind::Blank,
                accepted: true,
                review: None,
                resolved_erasure: false,
                margin: blank_max - t1,
                theta_sel,
            };
        }
        return review(GroupKind::Blank, ReviewReason::Hysteresis, theta_sel);
    }

    let strong: Vec<usize> = (0..k).filter(|&i| d[i] >= theta_strong).collect();
    let sel: Vec<usize> = (0..k).filter(|&i| d[i] >= theta_sel).collect();

    // Multi.
    if strong.len() >= 2 {
        let kind = GroupKind::Multi(strong.iter().map(|&i| i as u8).collect());
        let min_strong = strong.iter().map(|&i| d[i]).min().unwrap_or(0);
        let max_other = (0..k)
            .filter(|i| !strong.contains(i))
            .map(|i| d[i])
            .max()
            .unwrap_or(i32::MIN / 2);
        if min_strong < theta_strong + h || max_other >= theta_sel - h {
            return review(kind, ReviewReason::UncertainSet, theta_sel);
        }
        let policy = match cfg.multi_review {
            MultiReview::Always => true,
            MultiReview::HighStakes => high_stakes,
            MultiReview::Never => false,
        };
        if policy {
            return review(kind, ReviewReason::MultiPolicy, theta_sel);
        }
        return GroupDecision {
            kind,
            accepted: true,
            review: None,
            resolved_erasure: false,
            margin: (min_strong - theta_strong).min(theta_sel - max_other),
            theta_sel,
        };
    }

    // Single (with the erasure rule).
    if sel.len() == 1 {
        let rho = i64::from(cfg.erasure_rho_permille);
        let gap = i32::from(cfg.erasure_gap);
        let rho_ok = i64::from(t2) * 1000 <= rho * i64::from(t1);
        let gap_ok = t1 - t2 >= gap;
        if rho_ok && gap_ok {
            let clear = t1 >= theta_sel + h
                && t2 < theta_sel - h
                && t1 - t2 >= gap + h
                && i64::from(t2 + h) * 1000 <= rho * i64::from(t1);
            let residue = t2 > i32::from(cfg.residue_log);
            if !clear {
                return review(GroupKind::Single(top), ReviewReason::Hysteresis, theta_sel);
            }
            if residue && (high_stakes || !cfg.erasure_rule) {
                return review(
                    GroupKind::Single(top),
                    ReviewReason::UnresolvedErasure,
                    theta_sel,
                );
            }
            let rho_slack = ((rho * i64::from(t1) - 1000 * i64::from(t2)) / 1000) as i32;
            return GroupDecision {
                kind: GroupKind::Single(top),
                accepted: true,
                review: None,
                resolved_erasure: residue,
                margin: (t1 - theta_sel)
                    .min(theta_sel - t2)
                    .min(t1 - t2 - gap)
                    .min(rho_slack),
                theta_sel,
            };
        }
        let why = if t2 >= i32::from(cfg.select_abs) {
            ReviewReason::TwoMarks
        } else if t2 > i32::from(cfg.residue_log) {
            ReviewReason::UnresolvedErasure
        } else {
            ReviewReason::WeakMark
        };
        return review(GroupKind::Single(top), why, theta_sel);
    }
    if sel.len() >= 2 {
        let kind = GroupKind::Multi(sel.iter().map(|&i| i as u8).collect());
        return review(kind, ReviewReason::TwoMarks, theta_sel);
    }
    // Nothing selected but something above blank: a weak or light-relative mark.
    let kind = if t1 < i32::from(cfg.select_abs) {
        GroupKind::Blank
    } else {
        GroupKind::Single(top)
    };
    review(kind, ReviewReason::WeakMark, theta_sel)
}

/// A margin score on the log-odds scale (not calibrated, §8.7 cascade deferred): accepted
/// decisions get `L_q + 40 · margin` nats (≤ 32), review decisions 0.
pub fn decision_logit(dec: &GroupDecision, lq: LogitMilli) -> LogitMilli {
    if dec.accepted {
        LogitMilli::from_nats((lq.nats() + 40.0 * f64::from(dec.margin) / 1024.0).min(32.0))
    } else {
        LogitMilli(0)
    }
}

/// Per-option selection pseudo-logits for ID-grid columns (§9.5 channel A input): confident
/// decisions give ±(L_q + margin bonus); in a review column, options above the blank threshold
/// get a score within (−L_q, L_q) from their distance to θ_sel and the rest −L_q.
pub fn option_logits(
    d: &[i32],
    dec: &GroupDecision,
    lq: LogitMilli,
    cfg: &DecisionConfig,
) -> Vec<LogitMilli> {
    let chosen: Vec<u8> = match &dec.kind {
        GroupKind::Blank => vec![],
        GroupKind::Single(i) => vec![*i],
        GroupKind::Multi(v) => v.clone(),
    };
    let strong = decision_logit(dec, lq);
    let cap = lq.nats() - 0.001;
    d.iter()
        .enumerate()
        .map(|(i, &v)| {
            if dec.accepted {
                if chosen.contains(&(i as u8)) {
                    strong
                } else {
                    LogitMilli(-strong.0)
                }
            } else if v >= i32::from(cfg.blank_max) {
                LogitMilli::from_nats(
                    (40.0 * f64::from(v - dec.theta_sel) / 1024.0).clamp(-cap, cap),
                )
            } else {
                LogitMilli(-lq.0)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Darkness fraction → 1/1024 units.
    fn q(x: f64) -> i32 {
        (x * 1024.0).round() as i32
    }

    fn dec(v: &[f64], level: f64) -> GroupDecision {
        let d: Vec<i32> = v.iter().map(|&x| q(x)).collect();
        decide_group(&d, q(level), &DecisionConfig::default(), false)
    }

    #[test]
    fn brief_cases() {
        // Solid fill.
        let r = dec(&[0.01, 0.86, -0.01, 0.0, 0.02], 0.85);
        assert_eq!((r.kind, r.accepted), (GroupKind::Single(1), true));
        // Light stipple alone on a light-pencil page (level ≈ 0.45).
        let r = dec(&[0.0, 0.0, 0.31, 0.01, 0.0], 0.45);
        assert_eq!((r.kind, r.accepted), (GroupKind::Single(2), true));
        // The same stipple on a solid-fill page: review, never silently blank.
        let r = dec(&[0.0, 0.0, 0.31, 0.01, 0.0], 0.90);
        assert_eq!(r.review, Some(ReviewReason::WeakMark));
        assert_eq!(r.kind, GroupKind::Single(2));
        // Erasure residue next to a fill: resolved, MRK-003 INFO.
        let r = dec(&[0.92, 0.18, 0.0, 0.01, 0.0], 0.9);
        assert_eq!(
            (r.kind.clone(), r.accepted, r.resolved_erasure),
            (GroupKind::Single(0), true, true)
        );
        // Erasure that reads below an empty bubble: plain single.
        let r = dec(&[-0.06, 0.0, 0.93, 0.0, 0.01], 0.9);
        assert_eq!(
            (r.kind.clone(), r.accepted, r.resolved_erasure),
            (GroupKind::Single(2), true, false)
        );
        // Multi.
        let r = dec(&[0.85, 0.0, 0.80, 0.0, 0.0], 0.85);
        assert_eq!((r.kind, r.accepted), (GroupKind::Multi(vec![0, 2]), true));
        // Blank.
        let r = dec(&[0.02, -0.01, 0.03, 0.0, 0.01], 0.85);
        assert_eq!((r.kind, r.accepted), (GroupKind::Blank, true));
        // Lone weak mark 0.08–0.20: review, proposed blank.
        let r = dec(&[0.0, 0.12, 0.0, 0.0, 0.0], 0.85);
        assert_eq!(
            (r.kind, r.review),
            (GroupKind::Blank, Some(ReviewReason::WeakMark))
        );
        // Two marks too close for the erasure rule: MRK-002.
        let r = dec(&[0.60, 0.45, 0.0, 0.0, 0.0], 0.9);
        assert_eq!(r.review, Some(ReviewReason::TwoMarks));
        // Residue too strong relative to a light fill: unresolved erasure (MRK-003 ERROR).
        let r = dec(&[0.28, 0.15, 0.0, 0.0, 0.0], 0.45);
        assert_eq!(r.review, Some(ReviewReason::UnresolvedErasure));
        // Multi with a third option near the selection threshold: uncertain set.
        let r = dec(&[0.85, 0.82, 0.50, 0.0, 0.0], 0.85);
        assert_eq!(r.review, Some(ReviewReason::UncertainSet));
    }

    #[test]
    fn hysteresis_and_high_stakes() {
        let cfg = DecisionConfig::default();
        // Just above θ_sel = 0.20 on a light page: within h → review.
        let d = [210, 0, 0, 0, 0];
        let r = decide_group(&d, 307, &cfg, false);
        assert_eq!(r.review, Some(ReviewReason::Hysteresis));
        // Blank just under 0.08: within h → review.
        let r = decide_group(&[78, 0, 0, 0, 0], 900, &cfg, false);
        assert_eq!(r.review, Some(ReviewReason::Hysteresis));
        // High stakes: an erasure is review-only.
        let d = [q(0.92), q(0.18), 0, 0, 0];
        let r = decide_group(&d, q(0.9), &cfg, true);
        assert_eq!(r.review, Some(ReviewReason::UnresolvedErasure));
        let mut always = cfg.clone();
        always.multi_review = MultiReview::Always;
        let r = decide_group(&[q(0.9), q(0.9), 0, 0, 0], q(0.9), &always, false);
        assert_eq!(r.review, Some(ReviewReason::MultiPolicy));
    }

    #[test]
    fn level_and_logits() {
        let cfg = DecisionConfig::default();
        assert_eq!(page_level([900, 100, 950, 920], &cfg), 920);
        assert_eq!(page_level([100, 50], &cfg), 1024);
        assert_eq!(page_level([260, 270], &cfg), 307);
        let d = [q(0.9), 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let r = decide_group(&d, q(0.9), &cfg, false);
        let l = option_logits(&d, &r, LogitMilli::STANDARD_LQ, &cfg);
        assert!(l[0] >= LogitMilli::STANDARD_LQ && l[1].0 <= -LogitMilli::STANDARD_LQ.0);
        let d = [q(0.12), 0, 0, 0];
        let r = decide_group(&d, q(0.9), &cfg, false);
        let l = option_logits(&d, &r, LogitMilli::STANDARD_LQ, &cfg);
        assert!(l[0].0.abs() < LogitMilli::STANDARD_LQ.0);
        assert_eq!(decision_logit(&r, LogitMilli::STANDARD_LQ), LogitMilli(0));
    }
}
