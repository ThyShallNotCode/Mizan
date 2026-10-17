//! Learning-outcome glue (§11): the CLO set, the item → CLO mapping in master space, the
//! offering and the attainment result with the TP-154 tables, from the exam definition and
//! the current score run (uncurved L3, §10.1 layer rule).

use crate::exam::{ExamDefinition, OutcomeSetup};
use crate::model::{EffectiveKey, Session};
use crate::score::{Scoring, master_item};
use omr_model::StudentKey;
use omr_outcomes::{
    A1Table, AssessmentEvidence, AttainmentPolicy, AttainmentResult, B1Table, Clo, CloPloMap,
    CloSet, ItemCloMap, MappingMode, MappingSet, Offering, RosterEntry, SectionInfo, Track,
    compute_attainment, tp154_a1, tp154_b1,
};
use omr_scoring::{RosterStatus, exact, parse_points};
use std::collections::{BTreeMap, BTreeSet};

/// Assessment ID used in mapping rows and evidence.
pub const ASSESSMENT: &str = "exam";

/// The CLO set of the exam.
pub fn clo_set(def: &ExamDefinition, o: &OutcomeSetup) -> CloSet {
    CloSet {
        course: def.course_code.clone(),
        version: 1,
        clos: o
            .clos
            .iter()
            .map(|c| {
                let mut x = Clo::new(&c.code, &c.title, c.domain.domain());
                x.text_ar = c.title_ar.clone();
                x
            })
            .collect(),
    }
}

/// CLO → PLO links (strength 2 unless stated).
pub fn clo_plo(o: &OutcomeSetup) -> Vec<CloPloMap> {
    o.clos
        .iter()
        .flat_map(|c| {
            c.plo
                .split([',', ';', ' '])
                .filter(|p| !p.trim().is_empty())
                .map(|p| CloPloMap::new(&c.code, p.trim(), 2))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The mapping set in master space: question q maps every form's copy of q.
pub fn mapping_set(
    def: &ExamDefinition,
    o: &OutcomeSetup,
    key: Option<&EffectiveKey>,
    n_items: usize,
) -> MappingSet {
    let n = usize::from(def.questions);
    let forms = match key {
        Some(k) if !k.shared_items => def.forms.len(),
        _ => 1,
    };
    let mut rows = Vec::new();
    for m in &o.mapping {
        let w = parse_points(&m.weight).unwrap_or_else(|_| omr_scoring::pts(1, 1));
        let q = usize::from(m.question).saturating_sub(1);
        for f in 0..forms {
            let item = match key {
                Some(k) => master_item(k, n, f, q),
                None => q,
            };
            if item < n_items.max(n) {
                let mut r = ItemCloMap::approved(ASSESSMENT, item, &m.clo, w);
                r.note = m.note.clone();
                rows.push(r);
            }
        }
    }
    MappingSet {
        version: 1,
        mode: if o.split {
            MappingMode::Split
        } else {
            MappingMode::Full
        },
        rows,
    }
}

/// Everything the outcomes screen and the TP-154 exports need.
#[derive(Clone, Debug)]
pub struct OutcomeReport {
    /// CLOs.
    pub clos: CloSet,
    /// Mapping.
    pub mapping: MappingSet,
    /// Policy.
    pub policy: AttainmentPolicy,
    /// Offering (roster, sections, evidence).
    pub offering: Offering,
    /// Result.
    pub result: AttainmentResult,
    /// TP-154 B.1.
    pub b1: B1Table,
    /// TP-154 A.1.
    pub a1: A1Table,
}

/// Computes attainment (and the TP-154 tables) for the current run.
pub fn compute(session: &Session, scoring: &Scoring) -> Option<OutcomeReport> {
    let def = &session.definition;
    let run = scoring.run.as_ref().ok()?;
    let a1 = tp154_a1(
        &scoring.grades().into_iter().flatten().collect::<Vec<_>>(),
        &omr_scoring::GradeScale::saudi(),
    );
    let o = def.outcomes.as_ref()?;
    let clos = clo_set(def, o);
    let mapping = mapping_set(def, o, session.key.as_ref(), scoring.forms.items.len());
    let policy = o.policy.policy();
    // Population: the roster, else every identified student of the run (rosterless).
    let mut roster: Vec<RosterEntry> = Vec::new();
    let mut sections: BTreeSet<String> = BTreeSet::new();
    if let Some(rows) = &def.roster {
        for r in rows {
            sections.insert(r.section.clone());
            roster.push(RosterEntry {
                student: StudentKey(omr_identity::normalize_id(&r.id)),
                section: r.section.clone(),
                status: RosterStatus::Enrolled,
                passing: None,
            });
        }
    } else {
        let mut seen = BTreeSet::new();
        for s in &run.students {
            if let Some(k) = &s.student
                && seen.insert(k.clone())
            {
                roster.push(RosterEntry {
                    student: k.clone(),
                    section: String::new(),
                    status: RosterStatus::Enrolled,
                    passing: None,
                });
            }
        }
        sections.insert(String::new());
    }
    let offering = Offering {
        course: def.course_code.clone(),
        term: def.term.clone(),
        credit_hours: 3,
        sections: sections
            .into_iter()
            .map(|id| SectionInfo {
                id,
                campus: "main".into(),
                track: Track::Mixed,
            })
            .collect(),
        roster,
        assessments: vec![AssessmentEvidence::from_run(
            ASSESSMENT,
            &def.title,
            exact(1, 1),
            run,
        )],
    };
    let result = compute_attainment(&offering, &clos, &mapping, &policy, &[]);
    let b1 = tp154_b1(
        &offering,
        &clos,
        &mapping,
        &clo_plo(o),
        &result,
        &policy,
        &BTreeMap::new(),
    );
    Some(OutcomeReport {
        clos,
        mapping,
        policy,
        offering,
        result,
        b1,
        a1,
    })
}

/// TP-154 A.1 without outcomes (grade distribution only).
pub fn a1_only(scoring: &Scoring) -> A1Table {
    tp154_a1(
        &scoring.grades().into_iter().flatten().collect::<Vec<_>>(),
        &omr_scoring::GradeScale::saudi(),
    )
}
