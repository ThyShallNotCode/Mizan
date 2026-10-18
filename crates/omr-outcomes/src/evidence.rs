//! Evidence inputs: per-assessment uncurved L3 scores with the §10.1 step-1 artefacts, plus
//! the offering's roster, sections and assessment weights (spec §11 preamble, §11.3).
//!
//! CLO attainment reads L3 and the step-1 artefacts (void list, choice-group selection,
//! excused set) and nothing else from L4–L6: curves, overrides and misconduct penalties are
//! excluded by construction.

use num_traits::{Signed, Zero};
use omr_model::StudentKey;
use omr_scoring::{
    AssessmentStatus, Counted, Exact, ItemState, Points, RosterStatus, ScoreRun, widen,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One item of an assessment as evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// Label as printed in master order (e.g. `Q12`).
    pub label: String,
    /// M_i.
    pub max: Points,
    /// Bonus item (§11.4 `bonus_items_as_evidence`).
    pub bonus: bool,
    /// Voided items are never CLO evidence (§10.5).
    pub voided: bool,
    /// SBA/TF/grid-in: splitting it across CLOs raises SCO-034.
    pub indivisible: bool,
}

/// One student's evidence on one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemEvidence {
    /// Counted: s⁺ = max(0, s) and whether a response was given (for bonus `attempted_only`).
    Counted {
        /// s⁺_{n,i}.
        score: Points,
        /// Non-blank response.
        attempted: bool,
    },
    /// Not evidence for this student (excused, unselected in a choice group, not presented).
    Excluded,
}

/// Per-assessment status of a student as evidence (§11.3 missing-data table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvidenceStatus {
    /// Sat the assessment.
    Present,
    /// Absent, excused, or the assessment was `NotScorable`: removed from A_{n,c}.
    Excused,
    /// Absent, unexcused: s = 0 on that assessment's items.
    Unexcused,
}

/// One student's evidence on one assessment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StudentEvidence {
    /// Stable (pseudonymous) student key used to join assessments.
    pub student: StudentKey,
    /// Status.
    pub status: EvidenceStatus,
    /// A `Pending` response or provisional total: the CLO result is provisional (SCO-033).
    pub provisional: bool,
    /// Evidence per item (same order as [`AssessmentEvidence::items`]).
    pub items: Vec<ItemEvidence>,
}

/// One assessment's evidence (one score run, or an imported column set).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssessmentEvidence {
    /// Assessment ID (joins mappings).
    pub id: String,
    /// Display name for "assessment methods" text, e.g. `Midterm`.
    pub name: String,
    /// W_a as a fraction of the course grade (Σ W_a = 1).
    pub weight: Exact,
    /// Direct evidence (exam, imported) or indirect (survey).
    pub direct: bool,
    /// Programme-level exit exam (feeds X_p via ItemOutcomeMap only).
    pub programme_level: bool,
    /// Items in master order.
    pub items: Vec<EvidenceItem>,
    /// Students.
    pub students: Vec<StudentEvidence>,
    /// Score-run ID (provenance).
    pub run_id: String,
    /// Key version scored under (SCO-030 skew check).
    pub key_version: u32,
    /// Policy version scored under (SCO-030 skew check).
    pub policy_version: u32,
    /// A common denominator of every M_i and counted score (the run's D), if known; lets the
    /// engine accumulate in integers. `None` for hand-built evidence (computed by scan).
    pub score_denom: Option<i64>,
}

impl AssessmentEvidence {
    /// Builds evidence from a score run (uncurved L3 plus step-1 artefacts).
    ///
    /// Rows without a bound student are skipped (candidate-number or anonymous modes cannot
    /// be joined across assessments, §11.3).
    pub fn from_run(id: &str, name: &str, weight: Exact, run: &ScoreRun) -> Self {
        let items = run
            .items
            .iter()
            .map(|m| EvidenceItem {
                label: m.label.clone(),
                max: m.max_points,
                bonus: m.bonus,
                voided: m.void.is_some(),
                indivisible: m.kind.is_indivisible(),
            })
            .collect();
        let mut students = Vec::new();
        for (n, s) in run.students.iter().enumerate() {
            let Some(student) = s.student.clone() else {
                continue;
            };
            let status = match s.status {
                AssessmentStatus::Absent { excused: false } => EvidenceStatus::Unexcused,
                AssessmentStatus::Absent { excused: true } | AssessmentStatus::NotScorable => {
                    EvidenceStatus::Excused
                }
                _ => EvidenceStatus::Present,
            };
            let mut provisional = s.status == AssessmentStatus::Provisional;
            let items = (0..run.items.len())
                .map(|i| match run.counted(n, i) {
                    Counted::Counted => {
                        let st = run.item_state(n, i);
                        if st == ItemState::Pending {
                            provisional = true;
                        }
                        let v = run.item_score(n, i);
                        ItemEvidence::Counted {
                            score: if v.is_negative() { Points::zero() } else { v },
                            attempted: st == ItemState::Scored,
                        }
                    }
                    Counted::Absent => ItemEvidence::Counted {
                        score: Points::zero(),
                        attempted: false,
                    },
                    _ => ItemEvidence::Excluded,
                })
                .collect();
            students.push(StudentEvidence {
                student,
                status,
                provisional,
                items,
            });
        }
        Self {
            id: id.into(),
            name: name.into(),
            weight,
            direct: true,
            programme_level: false,
            items,
            students,
            run_id: run.run_id.clone(),
            key_version: run.key_version,
            policy_version: run.policy_version,
            score_denom: Some(run.l3.denom),
        }
    }

    /// A common denominator of every M_i and counted score: `score_denom` or a scan.
    pub fn common_denom(&self) -> Option<i128> {
        if let Some(d) = self.score_denom {
            return Some(i128::from(d));
        }
        let mut d: i128 = 1;
        let mut add = |den: i64| -> Option<()> {
            let den = i128::from(den);
            if d % den != 0 {
                d = omr_scoring::num::lcm_i128(d, den).filter(|x| *x <= i128::from(i64::MAX))?;
            }
            Some(())
        };
        for it in &self.items {
            add(*it.max.denom())?;
        }
        for s in &self.students {
            for e in &s.items {
                if let ItemEvidence::Counted { score, .. } = e {
                    add(*score.denom())?;
                }
            }
        }
        Some(d)
    }

    /// M_a: Σ M_i over non-bonus, non-voided items (§11.2 g_i = W_a·M_i/M_a).
    pub fn max_non_bonus(&self) -> Exact {
        self.items
            .iter()
            .filter(|i| !i.bonus && !i.voided)
            .map(|i| widen(&i.max))
            .sum()
    }

    /// Item grade share g_i = W_a·M_i/M_a (0 for bonus and voided items).
    pub fn grade_share(&self, i: usize) -> Exact {
        let ma = self.max_non_bonus();
        match self.items.get(i) {
            Some(it) if !it.bonus && !it.voided && !ma.is_zero() => {
                self.weight * widen(&it.max) / ma
            }
            _ => Exact::zero(),
        }
    }
}

/// Section track (§11.2 "Section attributes").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Track {
    /// Male section.
    Male,
    /// Female section.
    Female,
    /// Mixed section.
    Mixed,
}

/// A section of an offering.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SectionInfo {
    /// Section ID.
    pub id: String,
    /// Campus: `main` or a branch code.
    pub campus: String,
    /// Track.
    pub track: Track,
}

/// One roster row of the offering.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RosterEntry {
    /// Student key.
    pub student: StudentKey,
    /// Section of record.
    pub section: String,
    /// Roster status (W, DN and E are excluded from N_c).
    pub status: RosterStatus,
    /// Passing course letter (for EAMU `eamu_population = passing`).
    pub passing: Option<bool>,
}

/// An offering (course × term) with its sections, roster and assessments (§11.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Offering {
    /// Course code.
    pub course: String,
    /// Term label (Hijri and Gregorian).
    pub term: String,
    /// Credit hours C_k.
    pub credit_hours: u32,
    /// Sections.
    pub sections: Vec<SectionInfo>,
    /// Roster.
    pub roster: Vec<RosterEntry>,
    /// Assessments (direct and indirect).
    pub assessments: Vec<AssessmentEvidence>,
}

impl Offering {
    /// Roster lookup by student.
    pub fn roster_map(&self) -> BTreeMap<&StudentKey, &RosterEntry> {
        self.roster.iter().map(|r| (&r.student, r)).collect()
    }

    /// Section lookup by ID.
    pub fn section(&self, id: &str) -> Option<&SectionInfo> {
        self.sections.iter().find(|s| s.id == id)
    }
}

/// Indirect evidence for one CLO from `survey` assessments (§11.3 "Direct + indirect").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndirectEvidence {
    /// CLO code.
    pub clo: String,
    /// Respondents with a value (N/A excluded, reverse-coded items already flipped).
    pub respondents: u32,
    /// Respondents at or above `indirect_cutoff`.
    pub at_or_above: u32,
}
