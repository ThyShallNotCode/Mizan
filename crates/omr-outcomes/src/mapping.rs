//! Item → CLO mappings (`ItemCloMap`) and item → PLO/SO mappings for exit exams
//! (`ItemOutcomeMap`), with the §11.2 validation lints (spec §11.2, §11.8).

use crate::evidence::{AssessmentEvidence, Offering};
use crate::framework::CloSet;
use num_traits::{One, Zero};
use omr_model::{Exception, Scope, Severity};
use omr_scoring::{Exact, Points, widen};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Approval status of a mapping row; official reports use `Approved` rows only (SCO-032).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MappingStatus {
    /// Work in progress.
    Draft,
    /// Imported or AI-suggested, awaiting approval.
    Proposed,
    /// Approved by a coordinator- or QA-scoped role.
    Approved,
    /// Superseded.
    Retired,
}

/// Where a mapping row came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MappingSource {
    /// Entered by hand.
    Manual,
    /// Remark import (weight 1, "needs review").
    RemarkImport,
    /// QTI metadata.
    Qti,
    /// AI suggestion (advisory).
    Ai,
    /// Carried forward from a previous term ("confirm").
    CarriedForward,
}

/// Full or split mode of a mapping set (§11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MappingMode {
    /// Every mapped CLO receives the item's whole evidence (default).
    #[default]
    Full,
    /// Σ_c w_{i,c} = 1 per item.
    Split,
}

/// One item → CLO evidence row (`ItemCloMap`, §11.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemCloMap {
    /// Assessment ID.
    pub assessment: String,
    /// Master item index within the assessment.
    pub item: usize,
    /// Target CLO code.
    pub clo: String,
    /// Target CLO version.
    pub clo_version: u32,
    /// w_{i,c} ≥ 0: relative strength of the item as evidence for the CLO.
    pub weight: Points,
    /// Free bilingual note, e.g. "parts b–c only".
    pub note: String,
    /// Content hash or URI of the item text, rubric or QTI item.
    pub evidence_uri: Option<String>,
    /// Why the item evidences the CLO (required for `approved` in accreditation mode).
    pub rationale: Option<String>,
    /// Revised-Bloom level 1–6 of the item (SCO-015).
    pub bloom_item: Option<u8>,
    /// Approval status.
    pub status: MappingStatus,
    /// Source.
    pub source: MappingSource,
    /// Approver.
    pub approved_by: Option<String>,
    /// Approval time (ISO 8601).
    pub approved_at: Option<String>,
    /// Row version.
    pub version: u32,
    /// Superseded row version.
    pub supersedes: Option<u32>,
}

impl ItemCloMap {
    /// An approved, manual row with weight `w`.
    pub fn approved(assessment: &str, item: usize, clo: &str, w: Points) -> Self {
        Self {
            assessment: assessment.into(),
            item,
            clo: clo.into(),
            clo_version: 1,
            weight: w,
            note: String::new(),
            evidence_uri: None,
            rationale: Some("aligned by the course coordinator".into()),
            bloom_item: None,
            status: MappingStatus::Approved,
            source: MappingSource::Manual,
            approved_by: Some("coordinator".into()),
            approved_at: None,
            version: 1,
            supersedes: None,
        }
    }
}

/// A versioned set of item → CLO rows.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MappingSet {
    /// Version.
    pub version: u32,
    /// Full or split.
    pub mode: MappingMode,
    /// Rows.
    pub rows: Vec<ItemCloMap>,
}

impl MappingSet {
    /// Rows for one assessment and CLO with w > 0.
    pub fn rows_for<'a>(
        &'a self,
        assessment: &'a str,
        clo: &'a str,
    ) -> impl Iterator<Item = &'a ItemCloMap> + 'a {
        self.rows.iter().filter(move |r| {
            r.assessment == assessment
                && r.clo == clo
                && r.weight > Points::zero()
                && r.status != MappingStatus::Retired
        })
    }

    /// Validation lints (§11.2, §11.8): SCO-011, SCO-012, SCO-013, SCO-015, SCO-032, SCO-034.
    /// `accreditation` also requires a rationale on approved rows (reported as SCO-032).
    pub fn validate(
        &self,
        offering: &Offering,
        clos: &CloSet,
        accreditation: bool,
    ) -> Vec<Exception> {
        let mut ex = Vec::new();
        let rep = |code: &str, sev: Severity, d: String| {
            Exception::must(code, sev, Scope::OutcomeReport).with_detail(d)
        };
        let live: Vec<&ItemCloMap> = self
            .rows
            .iter()
            .filter(|r| r.status != MappingStatus::Retired)
            .collect();
        let find = |a: &str| offering.assessments.iter().find(|x| x.id == a);
        for r in &live {
            if r.weight < Points::zero() {
                ex.push(rep(
                    "SCO-011",
                    Severity::Error,
                    format!(
                        "{} item {} → {}: negative weight",
                        r.assessment, r.item, r.clo
                    ),
                ));
            }
            match clos.clo(&r.clo) {
                None => ex.push(rep(
                    "SCO-011",
                    Severity::Error,
                    format!("{} item {}: unknown CLO {}", r.assessment, r.item, r.clo),
                )),
                Some(c) if c.retired || c.version != r.clo_version => ex.push(
                    rep(
                        "SCO-011",
                        Severity::Error,
                        format!(
                            "{} item {} → {} v{}: CLO version not in force",
                            r.assessment, r.item, r.clo, r.clo_version
                        ),
                    )
                    .with_sub_reason("retired_clo_version"),
                ),
                Some(c) => {
                    if let (Some(ib), Some(cb)) = (r.bloom_item, c.bloom)
                        && ib < cb
                    {
                        ex.push(rep(
                            "SCO-015",
                            Severity::Warn,
                            format!(
                                "{} item {} (Bloom {ib}) below {} (Bloom {cb})",
                                r.assessment, r.item, r.clo
                            ),
                        ));
                    }
                }
            }
            if find(&r.assessment).is_none_or(|a| r.item >= a.items.len()) {
                ex.push(rep(
                    "SCO-011",
                    Severity::Error,
                    format!(
                        "{} item {}: evidence source does not exist",
                        r.assessment, r.item
                    ),
                ));
            }
            if r.status != MappingStatus::Approved {
                ex.push(rep(
                    "SCO-032",
                    Severity::Warn,
                    format!(
                        "{} item {} → {} is {:?}: draft-mappings watermark",
                        r.assessment, r.item, r.clo, r.status
                    ),
                ));
            } else if accreditation && r.rationale.as_deref().is_none_or(str::is_empty) {
                ex.push(rep(
                    "SCO-032",
                    Severity::Warn,
                    format!(
                        "{} item {} → {}: approved without a rationale",
                        r.assessment, r.item, r.clo
                    ),
                ));
            }
        }
        // Per item: split sums and indivisible splits.
        let mut per_item: BTreeMap<(&str, usize), Vec<&ItemCloMap>> = BTreeMap::new();
        for r in &live {
            per_item.entry((&r.assessment, r.item)).or_default().push(r);
        }
        for ((a, i), rows) in &per_item {
            let positive: Vec<_> = rows.iter().filter(|r| r.weight > Points::zero()).collect();
            if self.mode == MappingMode::Split {
                let sum: Points = rows.iter().map(|r| r.weight).sum();
                if sum != Points::one() {
                    ex.push(rep(
                        "SCO-011",
                        Severity::Error,
                        format!("{a} item {i}: split weights sum to {sum}, not 1"),
                    ));
                }
                let indivisible = find(a)
                    .and_then(|x| x.items.get(*i))
                    .is_some_and(|it| it.indivisible);
                if indivisible && positive.len() > 1 {
                    ex.push(rep(
                        "SCO-034",
                        Severity::Info,
                        format!(
                            "{a} item {i}: one indivisible item split across {} CLOs; map a \
                             primary CLO or sub-parts",
                            positive.len()
                        ),
                    ));
                }
            }
        }
        // Coverage: scored items with no CLO, CLOs with no (non-voided) evidence.
        for a in offering
            .assessments
            .iter()
            .filter(|a| a.direct && !a.programme_level)
        {
            for (i, it) in a.items.iter().enumerate() {
                if !it.voided && !per_item.contains_key(&(a.id.as_str(), i)) {
                    ex.push(rep(
                        "SCO-013",
                        Severity::Warn,
                        format!("{} {}: scored item with no CLO mapping", a.name, it.label),
                    ));
                }
            }
        }
        for c in clos.clos.iter().filter(|c| !c.retired) {
            let has = live.iter().any(|r| {
                r.clo == c.code
                    && r.weight > Points::zero()
                    && find(&r.assessment)
                        .and_then(|a| a.items.get(r.item))
                        .is_some_and(|it| !it.voided)
            });
            if !has {
                ex.push(rep(
                    "SCO-012",
                    Severity::Error,
                    format!(
                        "{}: no evidence (or all voided): \"not assessed\", never 0 %",
                        c.code
                    ),
                ));
            }
        }
        // Bloom share below level (heuristic): < 50 % of evidence weight at or above the CLO level.
        for c in clos.clos.iter().filter(|c| c.bloom.is_some()) {
            let cb = c.bloom.unwrap_or(1);
            let (mut total, mut at) = (Exact::zero(), Exact::zero());
            for r in live.iter().filter(|r| r.clo == c.code) {
                let Some(a) = find(&r.assessment) else {
                    continue;
                };
                let e = widen(&r.weight) * a.grade_share(r.item);
                total += e;
                if r.bloom_item.is_some_and(|b| b >= cb) {
                    at += e;
                }
            }
            if !total.is_zero() && at * Exact::from_integer(2) < total {
                ex.push(
                    rep(
                        "SCO-015",
                        Severity::Warn,
                        format!("{}: assessed below stated level", c.code),
                    )
                    .with_sub_reason("below_level_share"),
                );
            }
        }
        ex
    }

    /// Distinct assessments that carry evidence for `clo`.
    pub fn assessments_for(&self, clo: &str) -> BTreeSet<&str> {
        self.rows
            .iter()
            .filter(|r| {
                r.clo == clo && r.weight > Points::zero() && r.status != MappingStatus::Retired
            })
            .map(|r| r.assessment.as_str())
            .collect()
    }
}

/// Item → PLO or framework outcome (exit exams, programme-level direct assessment; §11.2).
/// Same fields and rules as [`ItemCloMap`], with a PLO/SO target; feeds only X_p.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemOutcomeMap {
    /// Assessment ID (a `programme_level` exam).
    pub assessment: String,
    /// Master item index.
    pub item: usize,
    /// PLO or framework-outcome code.
    pub outcome: String,
    /// Target version.
    pub outcome_version: u32,
    /// w ≥ 0.
    pub weight: Points,
    /// Note.
    pub note: String,
    /// Rationale.
    pub rationale: Option<String>,
    /// Approval status.
    pub status: MappingStatus,
}

/// Exam-level helper: (assessment, item) pairs that evidence any of `clos`.
pub fn items_for_clos<'a>(
    mapping: &'a MappingSet,
    clos: &'a BTreeSet<String>,
) -> BTreeSet<(&'a str, usize)> {
    mapping
        .rows
        .iter()
        .filter(|r| {
            clos.contains(&r.clo) && r.weight > Points::zero() && r.status != MappingStatus::Retired
        })
        .map(|r| (r.assessment.as_str(), r.item))
        .collect()
}

/// Evidence weight e_{i,c} = w_{i,c}·g_i summed over a CLO's rows (thin-evidence share).
pub fn evidence_share(mapping: &MappingSet, offering: &Offering, clo: &str) -> Exact {
    mapping
        .rows
        .iter()
        .filter(|r| r.clo == clo && r.status != MappingStatus::Retired)
        .filter_map(|r| {
            let a: &AssessmentEvidence =
                offering.assessments.iter().find(|a| a.id == r.assessment)?;
            Some(widen(&r.weight) * a.grade_share(r.item))
        })
        .sum()
}
