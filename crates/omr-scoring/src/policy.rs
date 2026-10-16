//! Scoring policies (spec §10.4 rules, §10.5 policies).

use crate::form::FormMap;
use crate::item::{ItemKind, SelectMode};
use crate::num::{Points, pts};
use num_traits::Zero;
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Multi-select partial-credit rules (§10.4). Credit c is a proportion; s = M_i·c.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MultiRule {
    /// 1 iff FP = FN = 0.
    Dichotomous,
    /// (TP + TN)/m_i.
    Mtf,
    /// PS1/t (Ripkey): TP/t if TP + FP ≤ t, else 0. Pick-N only.
    Ps1t,
    /// PS50: 1 if TP = t and FP = 0; ½ if 2·TP ≥ t and TP + FP ≤ t; else 0. Pick-N only.
    Ps50,
    /// max(0, TP/t − FP/(m_i − t)); the FP term is 0 when m_i = t.
    PlusMinus,
}

impl MultiRule {
    /// The default rule for a select mode: Pick-N → PS1/t; select-all → plus/minus.
    pub fn default_for(mode: SelectMode) -> Self {
        match mode {
            SelectMode::PickN(_) => MultiRule::Ps1t,
            SelectMode::SelectAll => MultiRule::PlusMinus,
        }
    }

    /// Exact credit for TP true positives and FP false positives with t keyed of m options.
    pub fn credit(self, tp: u32, fp: u32, t: u32, m: u32) -> Points {
        let (tp, fp, t, m) = (i64::from(tp), i64::from(fp), i64::from(t), i64::from(m));
        let one = Points::from_integer(1);
        let zero = Points::zero();
        if t == 0 {
            return zero;
        }
        let fneg = t - tp;
        let tn = m - t - fp;
        match self {
            MultiRule::Dichotomous => {
                if fp == 0 && fneg == 0 {
                    one
                } else {
                    zero
                }
            }
            MultiRule::Mtf => pts(tp + tn, m),
            MultiRule::Ps1t => {
                if tp + fp <= t {
                    pts(tp, t)
                } else {
                    zero
                }
            }
            MultiRule::Ps50 => {
                if tp == t && fp == 0 {
                    one
                } else if 2 * tp >= t && tp + fp <= t {
                    pts(1, 2)
                } else {
                    zero
                }
            }
            MultiRule::PlusMinus => {
                let fp_term = if m == t { zero } else { pts(fp, m - t) };
                let v = pts(tp, t) - fp_term;
                if v < zero { zero } else { v }
            }
        }
    }
}

/// Negative marking for single-answer items (§10.5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum NegativeMarking {
    /// No penalty (default).
    #[default]
    Off,
    /// A wrong answer earns −p_i (a proportion of M_i): `default` unless overridden per item.
    Penalty {
        /// Penalty for items without an override.
        default: Points,
        /// Per-item penalties.
        per_item: BTreeMap<usize, Points>,
    },
    /// Formula scoring: wrong = −a_i/(m_i − a_i), so blind guessing gains 0 in expectation.
    Formula,
}

/// How a confirmed multi-mark on a single-answer item scores (§10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MultiOnSba {
    /// Invalid = 0 (default).
    #[default]
    InvalidZero,
    /// max marked credit ÷ #set.
    PartialMaxOverCount,
    /// Scored as a wrong answer (penalty under negative marking).
    Wrong,
}

/// Penalty floors applied at L4 step 3 (§10.1, §10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Floors {
    /// Each item ≥ 0.
    pub item: bool,
    /// Each section ≥ 0.
    pub section: bool,
    /// The total ≥ 0 (default).
    pub total: bool,
}

impl Default for Floors {
    fn default() -> Self {
        Self {
            item: false,
            section: false,
            total: true,
        }
    }
}

impl Floors {
    /// No floor at all (SCO-019 with negative marking).
    pub const NONE: Floors = Floors {
        item: false,
        section: false,
        total: false,
    };

    /// True if any floor is active.
    pub fn any(&self) -> bool {
        self.item || self.section || self.total
    }
}

/// A section of an assessment (§10.5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Section {
    /// Display name.
    pub name: String,
    /// Master items in the section (sections partition at most once per item).
    pub items: Vec<usize>,
    /// Speeded: the trailing blank run in printed order is "not reached" (SCO-024).
    pub speeded: bool,
}

/// Choice-group selection mode (§10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChoiceMode {
    /// The q_G highest s_i, ties to the lower master position.
    Best,
    /// The first q_G non-blank items in printed order (π_f).
    FirstAttempted,
}

/// "Answer q_G of these items" (§10.5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChoiceGroup {
    /// Display name.
    pub name: String,
    /// Member master items (equal M_i, else SCO-004 `choice_group`).
    pub items: Vec<usize>,
    /// q_G.
    pub count: usize,
    /// Selection mode.
    pub mode: ChoiceMode,
}

/// A published scoring policy version (§10.5), consumed by every score run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PolicyVersion {
    /// Monotone version number.
    pub version: u32,
    /// Per-item multi-select rule overrides (default per [`MultiRule::default_for`]).
    pub multi_rules: BTreeMap<usize, MultiRule>,
    /// Negative marking for single-answer items.
    pub negative: NegativeMarking,
    /// Confirmed multi on single-answer items.
    pub multi_on_sba: MultiOnSba,
    /// Penalty floors.
    pub floors: Floors,
    /// Bonus items: numerator only; the 100 % cap applies at L4 step 5.
    pub bonus: BTreeSet<usize>,
    /// Sections.
    pub sections: Vec<Section>,
    /// Choice groups.
    pub choice_groups: Vec<ChoiceGroup>,
}

impl Default for PolicyVersion {
    fn default() -> Self {
        Self {
            version: 1,
            multi_rules: BTreeMap::new(),
            negative: NegativeMarking::Off,
            multi_on_sba: MultiOnSba::InvalidZero,
            floors: Floors::default(),
            bonus: BTreeSet::new(),
            sections: Vec::new(),
            choice_groups: Vec::new(),
        }
    }
}

impl PolicyVersion {
    /// The multi-select rule in force for item `i` of select mode `mode`.
    pub fn rule_for(&self, i: usize, mode: SelectMode) -> MultiRule {
        self.multi_rules
            .get(&i)
            .copied()
            .unwrap_or_else(|| MultiRule::default_for(mode))
    }

    /// Wrong-answer penalty proportion for item `i` (positive number), or `None` for formula
    /// scoring / no penalty.
    pub fn penalty_for(&self, i: usize) -> Option<Points> {
        match &self.negative {
            NegativeMarking::Penalty { default, per_item } => {
                Some(per_item.get(&i).copied().unwrap_or(*default))
            }
            _ => None,
        }
    }

    /// Policy lint (§10.4, §10.5): SCO-019 (WARN) for an exploitable policy (PS1/t or PS50 on a
    /// select-all item, an uncapped MTF rule on a Pick-N item, negative marking with no
    /// floor); SCO-004 (BLOCKER, sub-reason `choice_group`) for unequal M_i within a choice
    /// group, and SCO-004 for a group, section or rule naming an unknown item.
    pub fn lint(&self, forms: &FormMap) -> Vec<Exception> {
        let mut ex = Vec::new();
        let sco019 =
            |d: String| Exception::must("SCO-019", Severity::Warn, Scope::Exam).with_detail(d);
        let sco004 = |d: String| {
            Exception::must("SCO-004", Severity::Blocker, Scope::Exam).with_detail(d)
        };
        let n = forms.items.len();
        for (i, it) in forms.items.iter().enumerate() {
            if let ItemKind::MultiSelect(mode) = it.kind {
                let rule = self.rule_for(i, mode);
                match (mode, rule) {
                    (SelectMode::SelectAll, MultiRule::Ps1t | MultiRule::Ps50) => {
                        ex.push(sco019(format!(
                            "item {}: {rule:?} is Pick-N only; use plus/minus on select-all",
                            it.label
                        )));
                    }
                    (SelectMode::PickN(_), MultiRule::Mtf) => ex.push(sco019(format!(
                        "item {}: MTF on a Pick-N item has no cap on marks; use PS1/t",
                        it.label
                    ))),
                    _ => {}
                }
            }
        }
        for &i in self.multi_rules.keys() {
            if i >= n {
                ex.push(sco004(format!("multi-select rule for unknown item {i}")));
            }
        }
        if self.negative != NegativeMarking::Off && !self.floors.any() {
            ex.push(sco019(
                "negative marking with no floor: set an item, section or total floor".into(),
            ));
        }
        for &b in &self.bonus {
            if b >= n {
                ex.push(sco004(format!("bonus item {b} does not exist")));
            }
        }
        let mut sectioned = BTreeSet::new();
        for s in &self.sections {
            for &i in &s.items {
                if i >= n {
                    ex.push(sco004(format!(
                        "section {}: unknown item {i}",
                        s.name
                    )));
                } else if !sectioned.insert(i) {
                    ex.push(sco004(format!(
                        "item {} is in more than one section",
                        forms.items[i].label
                    )));
                }
            }
        }
        for g in &self.choice_groups {
            if g.items.iter().any(|&i| i >= n) {
                ex.push(sco004(format!("choice group {}: unknown item", g.name)));
                continue;
            }
            let maxima: BTreeSet<Points> =
                g.items.iter().map(|&i| forms.items[i].max_points).collect();
            if maxima.len() > 1 {
                ex.push(
                    sco004(format!(
                        "choice group {}: members have unequal M_i",
                        g.name
                    ))
                    .with_sub_reason("choice_group"),
                );
            }
            if g.count == 0 || g.count > g.items.len() {
                ex.push(
                    sco004(format!(
                        "choice group {}: q_G = {} of {} members",
                        g.name,
                        g.count,
                        g.items.len()
                    ))
                    .with_sub_reason("choice_group"),
                );
            }
        }
        ex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_rules_hand_oracle() {
        // m = 5 options, t = 2 keyed. Response marks one key and one distractor:
        // TP = 1, FP = 1, FN = 1, TN = 2.
        let (tp, fp, t, m) = (1, 1, 2, 5);
        assert_eq!(MultiRule::Dichotomous.credit(tp, fp, t, m), pts(0, 1));
        assert_eq!(MultiRule::Mtf.credit(tp, fp, t, m), pts(3, 5)); // (1 + 2)/5
        assert_eq!(MultiRule::Ps1t.credit(tp, fp, t, m), pts(1, 2)); // TP + FP = 2 <= 2
        assert_eq!(MultiRule::Ps50.credit(tp, fp, t, m), pts(1, 2)); // 2*1 >= 2, 2 <= 2
        // plus/minus: max(0, 1/2 - 1/3) = 1/6
        assert_eq!(MultiRule::PlusMinus.credit(tp, fp, t, m), pts(1, 6));
        // Over-marking: TP = 2, FP = 1 (3 marks > t): PS1/t -> 0, PS50 -> 0, MTF = 4/5.
        assert_eq!(MultiRule::Ps1t.credit(2, 1, 2, 5), pts(0, 1));
        assert_eq!(MultiRule::Ps50.credit(2, 1, 2, 5), pts(0, 1));
        assert_eq!(MultiRule::Mtf.credit(2, 1, 2, 5), pts(4, 5));
        assert_eq!(MultiRule::PlusMinus.credit(2, 1, 2, 5), pts(2, 3)); // 1 - 1/3
        // Perfect response.
        for r in [
            MultiRule::Dichotomous,
            MultiRule::Mtf,
            MultiRule::Ps1t,
            MultiRule::Ps50,
            MultiRule::PlusMinus,
        ] {
            assert_eq!(r.credit(2, 0, 2, 5), pts(1, 1), "{r:?}");
        }
        // Plus/minus floors at 0 and drops the FP term when m = t.
        assert_eq!(MultiRule::PlusMinus.credit(0, 3, 2, 5), pts(0, 1));
        assert_eq!(MultiRule::PlusMinus.credit(3, 0, 3, 3), pts(1, 1));
    }
}
