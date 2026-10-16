//! L4: grade adjustments in the normative order of spec §10.1 (voids, excused items and
//! choice groups fix the counted set; x%; floors; curve; clamp and bonus cap; no-decrease;
//! individual override; hold-harmless).

use crate::curve::{CurveSpec, FrozenCurve};
use crate::form::FormMap;
use crate::l3::{ItemScoreMatrix, ItemState};
use crate::num::{Exact, clamp, cmp_exact, reduce};
use crate::policy::{ChoiceMode, PolicyVersion};
use crate::response::ResponseMatrix;
use num_traits::Zero;
use omr_model::exceptions::Target;
use omr_model::{BookletSerial, Exception, FormCode, Scope, Severity, StudentKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Void policies (§10.5). A key correction is not a void.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VoidMode {
    /// The item leaves the counted set (default).
    Drop,
    /// Full credit to every student whose form presented it, blanks included.
    FullCredit,
    /// Full credit for any non-blank response (rarely defensible).
    CreditAny,
}

/// `ItemVoided` (L4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Void {
    /// Master item.
    pub item: usize,
    /// Policy.
    pub mode: VoidMode,
    /// Mandatory reason code.
    pub reason: String,
}

/// `IndividualOverride(student, Δ, reason, approver)` (L4 step 7); never CLO evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndividualOverride {
    /// Booklet the override applies to.
    pub booklet: BookletSerial,
    /// Δ in percentage points.
    pub delta_pp: Exact,
    /// Reason.
    pub reason: String,
    /// Approver.
    pub approver: String,
}

/// A curve to fit now, or frozen numbers to re-apply unchanged (§10.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Curve {
    /// Fit on this run's eligible cohort, then freeze.
    Fit(CurveSpec),
    /// Re-apply published numbers (later key versions, voids, sittings).
    Frozen(FrozenCurve),
}

/// L4 inputs (`adj` in the §10.1 signature).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Adjustments {
    /// Voided items.
    pub voids: Vec<Void>,
    /// Excused items per booklet (accommodations, lost pages): leave numerator and denominator.
    pub excused: BTreeMap<BookletSerial, BTreeSet<usize>>,
    /// Individual overrides.
    pub overrides: Vec<IndividualOverride>,
    /// Curve or equating.
    pub curve: Option<Curve>,
    /// `hold_harmless`: last published finals; final = max(step 7, last published).
    pub hold_harmless: BTreeMap<BookletSerial, Exact>,
}

/// Per-assessment status of one student (§10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssessmentStatus {
    /// Scored, all items resolved.
    Scored,
    /// A counted item is `Pending`: the release gate refuses it (§13.4).
    Provisional,
    /// Zero counted non-bonus maximum: treated as an excused absence at L5.
    NotScorable,
    /// Absent (not 0).
    Absent {
        /// Excused absence.
        excused: bool,
    },
}

/// Why an item does or does not count for one student (the §10.1 step-1 artefacts that CLO
/// attainment reads, §11.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Counted {
    /// Counted in the grade and as CLO evidence.
    Counted = 0,
    /// Voided with full or any-response credit: counts for the grade, never CLO evidence.
    VoidedCredited = 1,
    /// Voided (dropped): out of numerator and denominator.
    Voided = 2,
    /// Excused for this student.
    Excused = 3,
    /// Not selected within a choice group: excused for CLO evidence.
    Unselected = 4,
    /// Not on the student's form, or not applicable.
    NotPresented = 5,
    /// The student is absent.
    Absent = 6,
}

impl Counted {
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0 => Counted::Counted,
            1 => Counted::VoidedCredited,
            2 => Counted::Voided,
            3 => Counted::Excused,
            4 => Counted::Unselected,
            5 => Counted::NotPresented,
            _ => Counted::Absent,
        }
    }

    /// Counted toward the grade (numerator and, unless bonus, denominator).
    pub fn in_grade(self) -> bool {
        matches!(self, Counted::Counted | Counted::VoidedCredited)
    }
}

/// Raw and maximum of one section for one student.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SectionScore {
    /// Section name.
    pub name: String,
    /// Σ counted s_i (after item and section floors).
    pub raw: Exact,
    /// Σ counted non-bonus M_i.
    pub max: Exact,
}

/// One student's assessment result through L4.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StudentResult {
    /// Booklet.
    pub booklet: BookletSerial,
    /// Bound student.
    pub student: Option<StudentKey>,
    /// Form.
    pub form: FormCode,
    /// Status.
    pub status: AssessmentStatus,
    /// Σ counted s_i including bonus, after floors (step 3).
    pub raw: Exact,
    /// Σ counted non-bonus M_i.
    pub max: Exact,
    /// x% after floors (step 3).
    pub step3: Option<Exact>,
    /// After curve, clamp and bonus cap (step 5).
    pub step5: Option<Exact>,
    /// Released value: override, clamp, hold-harmless (step 8).
    pub final_pct: Option<Exact>,
    /// Section breakdown.
    pub sections: Vec<SectionScore>,
    /// An individual override was applied.
    pub overridden: bool,
}

/// Output of L4: per-student results, per-cell counted flags, the frozen curve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct L4Output {
    /// Results in row order.
    pub students: Vec<StudentResult>,
    /// Counted flags, column-major like the L3 matrix.
    pub counted: Vec<u8>,
    /// The curve applied (fitted now or re-applied), if any.
    pub curve: Option<FrozenCurve>,
    /// SCO-016 findings.
    pub exceptions: Vec<Exception>,
}

impl L4Output {
    /// Counted flag of (n, i).
    pub fn counted(&self, n_rows: usize, n: usize, i: usize) -> Counted {
        Counted::from_u8(self.counted[i * n_rows + n])
    }
}

/// Runs L4 for every row.
pub fn apply_l4(
    rm: &ResponseMatrix,
    l3: &ItemScoreMatrix,
    max_scaled: &[i64],
    forms: &FormMap,
    policy: &PolicyVersion,
    adj: &Adjustments,
) -> L4Output {
    let n_rows = rm.n_rows;
    let n_items = rm.n_items;
    let d = i128::from(l3.denom);
    let mut counted = vec![Counted::Counted as u8; n_rows * n_items];
    let void_mode: Vec<Option<VoidMode>> = {
        let mut v = vec![None; n_items];
        for x in &adj.voids {
            if let Some(slot) = v.get_mut(x.item) {
                *slot = Some(x.mode);
            }
        }
        v
    };
    let is_bonus: Vec<bool> = (0..n_items).map(|i| policy.bonus.contains(&i)).collect();
    let section_of: Vec<Option<usize>> = {
        let mut v = vec![None; n_items];
        for (s, sec) in policy.sections.iter().enumerate() {
            for &i in &sec.items {
                if let Some(slot) = v.get_mut(i) {
                    *slot = Some(s);
                }
            }
        }
        v
    };
    let printed_pos: Vec<Vec<Option<usize>>> = forms
        .forms
        .iter()
        .map(|f| f.positions_of_items(n_items))
        .collect();
    let mut overrides: BTreeMap<BookletSerial, Exact> = BTreeMap::new();
    for o in &adj.overrides {
        *overrides.entry(o.booklet).or_default() += o.delta_pp;
    }
    let hundred = Exact::from_integer(100);
    let mut students = Vec::with_capacity(n_rows);
    let mut adj_scores: Vec<i64> = vec![0; n_items];
    let n_sections = policy.sections.len();
    for (n, row) in rm.rows.iter().enumerate() {
        let form = forms.forms[row.form].code;
        if let Some(excused) = row.absent {
            for i in 0..n_items {
                counted[i * n_rows + n] = Counted::Absent as u8;
            }
            students.push(StudentResult {
                booklet: row.booklet,
                student: row.student.clone(),
                form,
                status: AssessmentStatus::Absent { excused },
                raw: Exact::zero(),
                max: Exact::zero(),
                step3: None,
                step5: None,
                final_pct: None,
                sections: Vec::new(),
                overridden: false,
            });
            continue;
        }
        let excused = adj.excused.get(&row.booklet);
        // Step 1: counted set and void-adjusted scores.
        for i in 0..n_items {
            let st = l3.state(n, i);
            let mut s = l3.scaled(n, i);
            let c = if !st.presented() {
                Counted::NotPresented
            } else if let Some(mode) = void_mode[i] {
                if is_bonus[i] {
                    Counted::Voided
                } else {
                    match mode {
                        VoidMode::Drop => Counted::Voided,
                        VoidMode::FullCredit => {
                            s = max_scaled[i];
                            Counted::VoidedCredited
                        }
                        VoidMode::CreditAny => {
                            s = if matches!(st, ItemState::Scored | ItemState::Pending) {
                                max_scaled[i]
                            } else {
                                0
                            };
                            Counted::VoidedCredited
                        }
                    }
                }
            } else if excused.is_some_and(|e| e.contains(&i)) {
                Counted::Excused
            } else {
                Counted::Counted
            };
            adj_scores[i] = s;
            counted[i * n_rows + n] = c as u8;
        }
        for g in &policy.choice_groups {
            let mut members: Vec<usize> = g
                .items
                .iter()
                .copied()
                .filter(|&i| i < n_items && counted[i * n_rows + n] == Counted::Counted as u8)
                .collect();
            let q = g.count.min(members.len());
            match g.mode {
                ChoiceMode::Best => {
                    members.sort_by(|&a, &b| adj_scores[b].cmp(&adj_scores[a]).then(a.cmp(&b)));
                }
                ChoiceMode::FirstAttempted => {
                    let pos = &printed_pos[row.form];
                    members.sort_by_key(|&i| {
                        let blank = !matches!(l3.state(n, i), ItemState::Scored);
                        (blank, pos[i].unwrap_or(usize::MAX))
                    });
                }
            }
            for &i in &members[q..] {
                counted[i * n_rows + n] = Counted::Unselected as u8;
            }
        }
        // Steps 2–3: sums with floors.
        let mut pending = false;
        let mut sec_raw = vec![0i128; n_sections];
        let mut sec_max = vec![0i128; n_sections];
        let mut loose_raw: i128 = 0;
        let mut max_total: i128 = 0;
        for i in 0..n_items {
            if !Counted::from_u8(counted[i * n_rows + n]).in_grade() {
                continue;
            }
            if l3.state(n, i) == ItemState::Pending && void_mode[i].is_none() {
                pending = true;
            }
            let mut s = i128::from(adj_scores[i]);
            if policy.floors.item && s < 0 {
                s = 0;
            }
            let m = if is_bonus[i] {
                0
            } else {
                i128::from(max_scaled[i])
            };
            max_total += m;
            match section_of[i] {
                Some(k) => {
                    sec_raw[k] += s;
                    sec_max[k] += m;
                }
                None => loose_raw += s,
            }
        }
        let mut raw_total = loose_raw;
        let mut sections = Vec::with_capacity(n_sections);
        for k in 0..n_sections {
            let mut r = sec_raw[k];
            if policy.floors.section && r < 0 {
                r = 0;
            }
            raw_total += r;
            sections.push(SectionScore {
                name: policy.sections[k].name.clone(),
                raw: reduce(r, d),
                max: reduce(sec_max[k], d),
            });
        }
        if policy.floors.total && raw_total < 0 {
            raw_total = 0;
        }
        let (status, step3) = if max_total == 0 {
            (AssessmentStatus::NotScorable, None)
        } else {
            let x = reduce(100 * raw_total, max_total);
            (
                if pending {
                    AssessmentStatus::Provisional
                } else {
                    AssessmentStatus::Scored
                },
                Some(x),
            )
        };
        students.push(StudentResult {
            booklet: row.booklet,
            student: row.student.clone(),
            form,
            status,
            raw: reduce(raw_total, d),
            max: reduce(max_total, d),
            step3,
            step5: None,
            final_pct: None,
            sections,
            overridden: overrides.contains_key(&row.booklet),
        });
    }
    // Step 4: curve, fitted on eligible step-3 values only.
    let mut exceptions = Vec::new();
    let curve = match &adj.curve {
        None => None,
        Some(Curve::Frozen(f)) => Some(f.clone()),
        Some(Curve::Fit(spec)) => {
            let cohort: Vec<Exact> = students
                .iter()
                .filter(|s| s.status == AssessmentStatus::Scored && !s.overridden)
                .filter_map(|s| s.step3)
                .collect();
            match spec.fit(&cohort) {
                Ok(f) => Some(f),
                Err(e) => {
                    exceptions.push(e);
                    None
                }
            }
        }
    };
    let zero = Exact::zero();
    use std::cmp::Ordering::{Greater, Less};
    for s in &mut students {
        let Some(x3) = s.step3 else { continue };
        let x4 = curve.as_ref().map_or(x3, |c| c.apply(&x3));
        // Step 5: clamp; the bonus cap at 100 % applies here only.
        let x5 = clamp(&x4, &zero, &hundred);
        // Step 6: no decrease below min(step 3, 100).
        let floor6 = if cmp_exact(&x3, &hundred) == Less {
            x3
        } else {
            hundred
        };
        if curve.is_some() && cmp_exact(&x5, &floor6) == Less {
            exceptions.push(
                Exception::must("SCO-016", Severity::Error, Scope::Exam)
                    .with_target(Target {
                        booklet: Some(s.booklet.0),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "curve lowers booklet {} from {} to {}",
                        s.booklet.0,
                        crate::num::decimal_string(&x3, 2, Default::default()),
                        crate::num::decimal_string(&x5, 2, Default::default())
                    )),
            );
        }
        // Step 7: individual override, then clamp.
        let x7 = match overrides.get(&s.booklet) {
            Some(delta) => clamp(&(x5 + delta), &zero, &hundred),
            None => x5,
        };
        // Step 8: hold-harmless.
        let x8 = match adj.hold_harmless.get(&s.booklet) {
            Some(prev) if cmp_exact(prev, &x7) == Greater => *prev,
            _ => x7,
        };
        s.step5 = Some(x5);
        s.final_pct = Some(x8);
    }
    L4Output {
        students,
        counted,
        curve,
        exceptions,
    }
}
