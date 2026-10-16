//! PLO and framework-outcome roll-ups (spec §11.3 "PLO formulas", §11.5) and the
//! CLO → PLO → SO crosswalk projection (§11.1, §11.7 "SO crosswalk").

use crate::attainment::{StudentClo, compute_attainment};
use crate::evidence::{AssessmentEvidence, Offering};
use crate::framework::{Clo, CloPloMap, CloSet, Crosswalk, Domain};
use crate::mapping::{ItemOutcomeMap, MappingSet, MappingStatus, items_for_clos};
use crate::policy::{AttainmentPolicy, Scale};
use num_traits::Zero;
use omr_model::{Exception, Scope, Severity, StudentKey};
use omr_scoring::Exact;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn links<'a>(map: &'a [CloPloMap], m_only: bool) -> impl Iterator<Item = &'a CloPloMap> + 'a {
    map.iter().filter(move |l| !m_only || l.strength >= 3)
}

/// Strength-weighted course-level PLO attainment A_{k,p} = Σ m·A_c / Σ m over the CLOs
/// that have a value (not-assessed CLOs are skipped, never 0) (§11.3).
pub fn plo_course_level(
    clo_values: &BTreeMap<String, Exact>,
    map: &[CloPloMap],
    m_only: bool,
) -> BTreeMap<String, Exact> {
    let mut acc: BTreeMap<String, (Exact, Exact)> = BTreeMap::new();
    for l in links(map, m_only) {
        if let Some(a) = clo_values.get(&l.clo) {
            let m = Exact::from_integer(l.strength.into());
            let e = acc.entry(l.plo.clone()).or_default();
            e.0 += m * a;
            e.1 += m;
        }
    }
    acc.into_iter()
        .filter(|(_, (_, d))| !d.is_zero())
        .map(|(p, (n, d))| (p, n / d))
        .collect()
}

/// Student-level P_{n,p} = Σ m·P_{n,c} / Σ m over the student's CLOs with evidence (§11.5).
pub fn plo_student_level(
    students: &[StudentClo],
    map: &[CloPloMap],
    m_only: bool,
) -> BTreeMap<(StudentKey, String), Exact> {
    let mut acc: BTreeMap<(StudentKey, String), (Exact, Exact)> = BTreeMap::new();
    for s in students.iter().filter(|s| s.included) {
        let Some(p) = s.p else { continue };
        for l in links(map, m_only).filter(|l| l.clo == s.clo) {
            let m = Exact::from_integer(l.strength.into());
            let e = acc.entry((s.student.clone(), l.plo.clone())).or_default();
            e.0 += m * p;
            e.1 += m;
        }
    }
    acc.into_iter()
        .filter(|(_, (_, d))| !d.is_zero())
        .map(|(k, (n, d))| (k, n / d))
        .collect()
}

/// One course's contribution to a credit-hour roll-up of PLO p.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CourseContribution {
    /// Course code.
    pub course: String,
    /// C_k.
    pub credit_hours: Exact,
    /// A_{k,p}.
    pub attainment: Exact,
    /// M_{k,p}: mapped share of the course grade evidencing p (∈ [0, 1]).
    pub mapped_share: Exact,
}

/// Credit-hour-weighted AR_p = Σ_k A_{k,p}·C_k·M_{k,p} / Σ_k C_k·M_{k,p} (UoM, §11.3).
pub fn credit_hour_rollup(contribs: &[CourseContribution]) -> Option<Exact> {
    let num: Exact = contribs
        .iter()
        .map(|c| c.attainment * c.credit_hours * c.mapped_share)
        .sum();
    let den: Exact = contribs
        .iter()
        .map(|c| c.credit_hours * c.mapped_share)
        .sum();
    (!den.is_zero()).then(|| num / den)
}

/// M_{k,p} = Σ g_i over the items of the offering that evidence p through any CLO, each item
/// counted once (§11.3).
pub fn mapped_grade_share(
    offering: &Offering,
    mapping: &MappingSet,
    clo_plo: &[CloPloMap],
    plo: &str,
) -> Exact {
    let clos: BTreeSet<String> = clo_plo
        .iter()
        .filter(|l| l.plo == plo)
        .map(|l| l.clo.clone())
        .collect();
    items_for_clos(mapping, &clos)
        .into_iter()
        .filter_map(|(a, i)| {
            let ev: &AssessmentEvidence = offering.assessments.iter().find(|x| x.id == a)?;
            Some(ev.grade_share(i))
        })
        .sum()
}

/// Exit-exam blend (1 − γ)·R_p + γ·X_p; γ = 0 (R_p) when no exit exam maps to p (§11.3).
pub fn exit_exam_blend(r: &Exact, x: Option<&Exact>, gamma: &Exact) -> Exact {
    match x {
        Some(x) => (Exact::from_integer(1) - gamma) * r + gamma * x,
        None => *r,
    }
}

/// X_p: exit-exam attainment per outcome under the policy's class method, from
/// [`ItemOutcomeMap`] evidence on `programme_level` assessments (§11.2, §11.3).
pub fn exit_exam_attainment(
    offering: &Offering,
    outcome_map: &[ItemOutcomeMap],
    policy: &AttainmentPolicy,
) -> BTreeMap<String, Exact> {
    // Reuse the CLO machinery: outcomes become pseudo-CLOs on a copy of the exit exams.
    let codes: BTreeSet<&str> = outcome_map.iter().map(|m| m.outcome.as_str()).collect();
    let clos = CloSet {
        course: "exit-exam".into(),
        version: 1,
        clos: codes
            .iter()
            .map(|c| Clo::new(c, c, Domain::of_code(c).unwrap_or(Domain::Skills)))
            .collect(),
    };
    let mapping = MappingSet {
        version: 1,
        mode: Default::default(),
        rows: outcome_map
            .iter()
            .filter(|m| m.status != MappingStatus::Retired)
            .map(|m| {
                let mut r = crate::mapping::ItemCloMap::approved(
                    &m.assessment,
                    m.item,
                    &m.outcome,
                    m.weight,
                );
                r.status = m.status;
                r
            })
            .collect(),
    };
    let exams = Offering {
        assessments: offering
            .assessments
            .iter()
            .filter(|a| a.programme_level)
            .map(|a| AssessmentEvidence {
                programme_level: false,
                ..a.clone()
            })
            .collect(),
        ..offering.clone()
    };
    // X_p is the direct value of the policy's deciding class method (average, threshold-
    // proportion, Taif per-question, levels or EAMU), exactly as for a CLO; indirect
    // evidence never enters it, and bootstrap draws are seeded from the exam's run IDs.
    let r = compute_attainment(&exams, &clos, &mapping, policy, &[]);
    r.clos
        .into_iter()
        .filter_map(|c| c.direct.map(|v| (c.clo, v)))
        .collect()
}

/// Framework-level SO_k = Σ_p x_{p,k}·PLO_p / Σ_p x_{p,k} (§11.5).
pub fn framework_rollup(
    plo_values: &BTreeMap<String, Exact>,
    crosswalk: &[Crosswalk],
) -> BTreeMap<String, Exact> {
    let mut acc: BTreeMap<String, (Exact, Exact)> = BTreeMap::new();
    for x in crosswalk {
        if let Some(v) = plo_values.get(&x.plo) {
            let s = Exact::from_integer(x.strength.into());
            let e = acc.entry(x.outcome.clone()).or_default();
            e.0 += s * v;
            e.1 += s;
        }
    }
    acc.into_iter()
        .filter(|(_, (_, d))| !d.is_zero())
        .map(|(k, (n, d))| (k, n / d))
        .collect()
}

/// One row of the CLO → PLO → SO crosswalk (§11.7 "SO crosswalk").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CrosswalkRow {
    /// CLO code.
    pub clo: String,
    /// NQF domain of the CLO.
    pub domain: Domain,
    /// PLO code.
    pub plo: String,
    /// CLO → PLO strength (I/P/M).
    pub clo_plo: char,
    /// Framework outcome, if the PLO is crosswalked.
    pub outcome: Option<String>,
    /// PLO → outcome strength.
    pub plo_outcome: Option<u8>,
}

/// Projects the stored chain CLO → PLO (K/S/V) → SO into rows; every accreditor view is a
/// projection of it (§11.1 "One crosswalk for both bodies").
pub fn crosswalk_rows(
    clos: &CloSet,
    clo_plo: &[CloPloMap],
    crosswalk: &[Crosswalk],
) -> Vec<CrosswalkRow> {
    let mut rows = Vec::new();
    for c in &clos.clos {
        for l in clo_plo.iter().filter(|l| l.clo == c.code) {
            let outs: Vec<&Crosswalk> = crosswalk.iter().filter(|x| x.plo == l.plo).collect();
            if outs.is_empty() {
                rows.push(CrosswalkRow {
                    clo: c.code.clone(),
                    domain: c.domain,
                    plo: l.plo.clone(),
                    clo_plo: l.ipm(),
                    outcome: None,
                    plo_outcome: None,
                });
            }
            for x in outs {
                rows.push(CrosswalkRow {
                    clo: c.code.clone(),
                    domain: c.domain,
                    plo: l.plo.clone(),
                    clo_plo: l.ipm(),
                    outcome: Some(x.outcome.clone()),
                    plo_outcome: Some(x.strength),
                });
            }
        }
    }
    rows
}

/// SCO-035 (ERROR) when the inputs of one blend or roll-up are on different scales (§11.5).
pub fn check_like_scales(scales: &[Scale]) -> Option<Exception> {
    let set: BTreeSet<_> = scales.iter().map(|s| *s as u8).collect();
    let eamu = scales.contains(&Scale::Eamu);
    (set.len() > 1 || eamu).then(|| {
        Exception::must("SCO-035", Severity::Error, Scope::OutcomeReport)
            .with_detail("roll-up inputs are not on one proportion or levels scale; policy refused")
    })
}
