//! L5 course aggregation and the SIS coursework/final split (spec §10.5 "Course policy",
//! §10.6 status codes).

use crate::grade::{GradeOutcome, GradeScale, StatusCode};
use crate::num::{Exact, LetterBasis, Points, RoundingPolicy, clamp, decimal_string, widen};
use num_rational::Ratio;
use num_traits::Zero;
use omr_model::{Exception, Scope, Severity, StudentKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which SIS column a component feeds (§10.5 `sis_split`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SisPart {
    /// Coursework mark.
    Coursework,
    /// Final-exam mark.
    Final,
}

/// A required minimum on a component (§10.5 `required`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Requirement {
    /// Minimum component percentage.
    pub min_pct: Exact,
    /// Letter the course grade is capped at when the requirement is not met.
    pub letter_cap: String,
}

/// One course component (assessment or imported column) with weight W_a.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Component {
    /// Assessment or column ID.
    pub id: String,
    /// W_a in percentage points; Σ W_a = 100 exactly (SCO-010).
    pub weight: Points,
    /// SIS column.
    pub sis_part: SisPart,
    /// Optional required minimum.
    pub required: Option<Requirement>,
}

/// `excused` rule (§10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExcusedRule {
    /// W′_a = W_a / Σ_{non-excused} W_b (default).
    #[default]
    Renormalize,
    /// Status IC until the make-up sitting releases.
    MakeupRequired,
}

/// `missing_work` rule (§10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MissingWork {
    /// Counts 0 (default).
    #[default]
    Zero,
    /// Status IC.
    Incomplete,
}

/// A course policy version (L5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CoursePolicy {
    /// Version number.
    pub version: u32,
    /// Components.
    pub components: Vec<Component>,
    /// Excused-absence rule.
    pub excused: ExcusedRule,
    /// Missing-work rule.
    pub missing_work: MissingWork,
    /// IC when the excused weight share exceeds this (initial ⅓).
    pub max_excused_share: Points,
    /// Extra-credit cap in percentage points (default 0).
    pub extra_credit_cap_pp: Points,
    /// Rounding (stage = Course).
    pub rounding: RoundingPolicy,
    /// Pass/fail course: NP/NF instead of a letter.
    pub pass_fail: bool,
}

impl CoursePolicy {
    /// A policy with the given components and every other parameter at its default.
    pub fn new(components: Vec<Component>) -> Self {
        Self {
            version: 1,
            components,
            excused: ExcusedRule::Renormalize,
            missing_work: MissingWork::Zero,
            max_excused_share: Ratio::new(1, 3),
            extra_credit_cap_pp: Points::zero(),
            rounding: RoundingPolicy::default(),
            pass_fail: false,
        }
    }

    /// SCO-010 (BLOCKER, Course) when Σ W_a ≠ 100 % exactly.
    pub fn validate(&self) -> Vec<Exception> {
        let sum: Points = self.components.iter().map(|c| c.weight).sum();
        if sum == Points::from_integer(100) {
            Vec::new()
        } else {
            vec![
                Exception::must("SCO-010", Severity::Blocker, Scope::Course).with_detail(format!(
                    "component weights sum to {sum} %, not 100 %: L5 publication refused"
                )),
            ]
        }
    }
}

/// A component's released result for one student (from its AssessmentResultSnapshot).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComponentMark {
    /// Released final percentage (L4 step 8).
    Scored(Exact),
    /// Not yet releasable (pending items).
    Provisional(Exact),
    /// Absent.
    Absent {
        /// Excused.
        excused: bool,
    },
    /// The assessment was not scorable for the student (treated as an excused absence).
    NotScorable,
    /// No value (unexcused missing work, or a blank imported cell).
    Missing,
}

/// Roster status from the snapshot (§9.2, §10.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum RosterStatus {
    /// Enrolled.
    #[default]
    Enrolled,
    /// Denied entry (attendance < 75 %): DN, counted as F; no final sitting.
    Denied,
    /// Withdrawn.
    Withdrawn,
    /// Exempted.
    Exempted,
    /// Course spans terms.
    InProgress,
}

/// One student's L5 input.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CourseStudent {
    /// Student.
    pub student: StudentKey,
    /// Roster status.
    pub status: RosterStatus,
    /// Component ID → mark.
    pub marks: BTreeMap<String, ComponentMark>,
    /// Extra credit in percentage points (capped by the policy).
    pub extra_credit_pp: Exact,
}

/// One student's L5/L6 result with the SIS split.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CourseResult {
    /// Student.
    pub student: StudentKey,
    /// Unrounded course total (%), when published.
    pub total: Option<Exact>,
    /// Coursework mark out of 100 (rounded at the course stage).
    pub coursework: Option<Exact>,
    /// Final mark out of 100 (rounded at the course stage).
    pub final_mark: Option<Exact>,
    /// coursework + final: the value the letter maps from (rounded basis).
    pub total_rounded: Option<Exact>,
    /// L6 outcome.
    pub grade: GradeOutcome,
    /// A required component minimum was missed (letter capped).
    pub requirement_not_met: bool,
    /// A component is provisional.
    pub provisional: bool,
}

/// Aggregates one student under `policy` and maps the letter on `scale` (§10.5, §10.6).
pub fn aggregate_student(
    policy: &CoursePolicy,
    s: &CourseStudent,
    scale: &GradeScale,
) -> CourseResult {
    let hundred = Exact::from_integer(100);
    let blank = |grade: GradeOutcome| CourseResult {
        student: s.student.clone(),
        total: None,
        coursework: None,
        final_mark: None,
        total_rounded: None,
        grade,
        requirement_not_met: false,
        provisional: false,
    };
    match s.status {
        RosterStatus::Withdrawn => return blank(GradeOutcome::Status(StatusCode::W)),
        RosterStatus::Exempted => return blank(GradeOutcome::Status(StatusCode::E)),
        RosterStatus::InProgress => return blank(GradeOutcome::Status(StatusCode::IP)),
        RosterStatus::Denied => {
            // Coursework exports as recorded; no final; DN counted as F (§10.6).
            let cw: Exact = policy
                .components
                .iter()
                .filter(|c| c.sis_part == SisPart::Coursework)
                .map(|c| match s.marks.get(&c.id) {
                    Some(ComponentMark::Scored(x) | ComponentMark::Provisional(x)) => {
                        widen(&c.weight) * x / hundred
                    }
                    _ => Exact::zero(),
                })
                .sum();
            let mut r = blank(GradeOutcome::Status(StatusCode::DN));
            r.coursework = Some(policy.rounding.round(&cw));
            return r;
        }
        RosterStatus::Enrolled => {}
    }
    let mut incomplete = false;
    let mut provisional = false;
    let mut requirement_not_met = false;
    let mut cap: Option<String> = None;
    let mut counted: Vec<(&Component, Exact)> = Vec::new();
    let mut excused_w = Exact::zero();
    for c in &policy.components {
        let w = widen(&c.weight);
        let mark = s.marks.get(&c.id).cloned().unwrap_or(ComponentMark::Missing);
        let x = match mark {
            ComponentMark::Scored(x) => Some(x),
            ComponentMark::Provisional(x) => {
                provisional = true;
                Some(x)
            }
            ComponentMark::Absent { excused: true } | ComponentMark::NotScorable => {
                match policy.excused {
                    ExcusedRule::Renormalize => excused_w += w,
                    ExcusedRule::MakeupRequired => incomplete = true,
                }
                None
            }
            ComponentMark::Absent { excused: false } | ComponentMark::Missing => {
                match policy.missing_work {
                    MissingWork::Zero => Some(Exact::zero()),
                    MissingWork::Incomplete => {
                        incomplete = true;
                        None
                    }
                }
            }
        };
        if let Some(x) = x {
            if let Some(req) = &c.required
                && x < req.min_pct
            {
                requirement_not_met = true;
                cap = Some(req.letter_cap.clone());
            }
            counted.push((c, x));
        }
    }
    let total_w: Exact = policy.components.iter().map(|c| widen(&c.weight)).sum();
    if total_w.is_zero() || excused_w / total_w > widen(&policy.max_excused_share) {
        incomplete = true;
    }
    let live_w = total_w - excused_w;
    if incomplete || live_w.is_zero() {
        let mut r = blank(GradeOutcome::Status(StatusCode::IC));
        r.provisional = provisional;
        return r;
    }
    // W′_a = W_a·100/Σ live W; part sums out of 100.
    let mut cw = Exact::zero();
    let mut fin = Exact::zero();
    for (c, x) in &counted {
        let wp = widen(&c.weight) * hundred / live_w;
        let v = wp * x / hundred;
        match c.sis_part {
            SisPart::Coursework => cw += v,
            SisPart::Final => fin += v,
        }
    }
    let extra = clamp(
        &s.extra_credit_pp,
        &Exact::zero(),
        &widen(&policy.extra_credit_cap_pp),
    );
    cw += extra;
    let total = clamp(&(cw + fin), &Exact::zero(), &hundred);
    let cw_r = policy.rounding.round(&cw);
    let fin_r = policy.rounding.round(&fin);
    let total_r = clamp(&(cw_r + fin_r), &Exact::zero(), &hundred);
    let basis = match policy.rounding.letter_basis {
        LetterBasis::Rounded => total_r,
        LetterBasis::Unrounded => total,
    };
    let mut grade = if policy.pass_fail {
        if basis >= Exact::from_integer(60) {
            GradeOutcome::Status(StatusCode::NP)
        } else {
            GradeOutcome::Status(StatusCode::NF)
        }
    } else {
        GradeOutcome::letter_for(scale, &basis).unwrap_or(GradeOutcome::Status(StatusCode::IC))
    };
    if let (Some(cap), GradeOutcome::Letter { .. }) = (&cap, &grade)
        && let Some(cap_band) = scale.bands.iter().position(|b| &b.letter == cap)
        && let Some(cur) = scale.bands.iter().position(|b| b.letter == grade.code())
        && cur > cap_band
    {
        let b = &scale.bands[cap_band];
        grade = GradeOutcome::Letter {
            letter: b.letter.clone(),
            arabic: b.arabic.clone(),
            gpa5: b.gpa5,
            gpa4: b.gpa4,
        };
    }
    CourseResult {
        student: s.student.clone(),
        total: Some(total),
        coursework: Some(cw_r),
        final_mark: Some(fin_r),
        total_rounded: Some(total_r),
        grade,
        requirement_not_met,
        provisional,
    }
}

/// Aggregates a course. Refused (SCO-010) when Σ W_a ≠ 100 %; grade release is unaffected.
pub fn aggregate_course(
    policy: &CoursePolicy,
    students: &[CourseStudent],
    scale: &GradeScale,
) -> Result<Vec<CourseResult>, Vec<Exception>> {
    let mut v = policy.validate();
    v.extend(scale.validate());
    if !v.is_empty() {
        return Err(v);
    }
    Ok(students
        .iter()
        .map(|s| aggregate_student(policy, s, scale))
        .collect())
}

/// SCO-017 (INFO, Student) boundary list (§10.2): students with |x − b| ≤ δ for a grade
/// boundary b, or whose rounded and unrounded totals map to different letters.
pub fn boundary_list(results: &[CourseResult], scale: &GradeScale, delta_pp: &Exact) -> Vec<Exception> {
    let bounds = scale.boundaries();
    let mut ex = Vec::new();
    for r in results {
        let (Some(x), Some(xr)) = (r.total, r.total_rounded) else {
            continue;
        };
        let near = bounds.iter().find(|b| {
            let d = x - **b;
            d <= *delta_pp && -d <= *delta_pp
        });
        let letters_differ = scale.band_for(&x).map(|b| &b.letter)
            != scale.band_for(&xr).map(|b| &b.letter);
        if near.is_some() || letters_differ {
            let why = match near {
                Some(b) => format!(
                    "within {} pp of boundary {}",
                    decimal_string(delta_pp, 2, Default::default()),
                    decimal_string(b, 2, Default::default())
                ),
                None => "rounded and unrounded totals map to different letters".into(),
            };
            ex.push(
                Exception::must("SCO-017", Severity::Info, Scope::Student).with_detail(format!(
                    "student {}: {} ({why})",
                    r.student,
                    decimal_string(&x, 4, Default::default())
                )),
            );
        }
    }
    ex
}
