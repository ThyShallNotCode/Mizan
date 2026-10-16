//! L6 grade scales and status codes (spec §10.6), with the built-in Saudi scale.

use crate::num::{Exact, Points, cmp_exact, pts};
use omr_model::{Exception, Scope, Severity};
use serde::{Deserialize, Serialize};

/// One half-open band [lo, hi) of a grade scale. `lo = None` is −∞, `hi = None` is +∞.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GradeBand {
    /// English letter, e.g. `A+`.
    pub letter: String,
    /// Arabic letter, e.g. `أ+`.
    pub arabic: String,
    /// Inclusive lower bound (%), `None` = −∞.
    pub lo: Option<Exact>,
    /// Exclusive upper bound (%), `None` = +∞.
    pub hi: Option<Exact>,
    /// GPA points on the 5-point scale.
    pub gpa5: Points,
    /// GPA points on the 4-point scale.
    pub gpa4: Points,
}

/// A versioned grade scale; a university variant is a new version, never an edit (§10.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GradeScale {
    /// Version number.
    pub version: u32,
    /// Name.
    pub name: String,
    /// Bands in ascending order of `lo`.
    pub bands: Vec<GradeBand>,
}

fn band(letter: &str, arabic: &str, lo: Option<i128>, hi: Option<i128>, g5: Points, g4: Points) -> GradeBand {
    GradeBand {
        letter: letter.into(),
        arabic: arabic.into(),
        lo: lo.map(Exact::from_integer),
        hi: hi.map(Exact::from_integer),
        gpa5: g5,
        gpa4: g4,
    }
}

impl GradeScale {
    /// The built-in Saudi scale (Council of Universities' Affairs, Study and Examination
    /// Regulations, 1444/9/1): A+ [95, ∞) … F (−∞, 60), with 5- and 4-point GPA columns.
    pub fn saudi() -> Self {
        Self {
            version: 1,
            name: "Saudi CUA 1444/9/1".into(),
            bands: vec![
                band("F", "هـ", None, Some(60), pts(1, 1), pts(0, 1)),
                band("D", "د", Some(60), Some(65), pts(2, 1), pts(1, 1)),
                band("D+", "د+", Some(65), Some(70), pts(5, 2), pts(3, 2)),
                band("C", "ج", Some(70), Some(75), pts(3, 1), pts(2, 1)),
                band("C+", "ج+", Some(75), Some(80), pts(7, 2), pts(5, 2)),
                band("B", "ب", Some(80), Some(85), pts(4, 1), pts(3, 1)),
                band("B+", "ب+", Some(85), Some(90), pts(9, 2), pts(7, 2)),
                band("A", "أ", Some(90), Some(95), pts(19, 4), pts(15, 4)),
                band("A+", "أ+", Some(95), None, pts(5, 1), pts(4, 1)),
            ],
        }
    }

    /// SCO-029 (BLOCKER) for a gap, an overlap, an open end or a non-increasing boundary over
    /// (−∞, +∞) (§10.6).
    pub fn validate(&self) -> Vec<Exception> {
        let mut ex = Vec::new();
        let sco029 =
            |d: String| Exception::must("SCO-029", Severity::Blocker, Scope::Exam).with_detail(d);
        if self.bands.is_empty() {
            ex.push(sco029("grade scale has no bands".into()));
            return ex;
        }
        if self.bands[0].lo.is_some() {
            ex.push(sco029("lowest band must start at −∞".into()));
        }
        if self.bands[self.bands.len() - 1].hi.is_some() {
            ex.push(sco029("highest band must end at +∞".into()));
        }
        for b in &self.bands {
            if let (Some(lo), Some(hi)) = (&b.lo, &b.hi)
                && lo >= hi
            {
                ex.push(sco029(format!("band {}: non-increasing boundary", b.letter)));
            }
        }
        for w in self.bands.windows(2) {
            match (&w[0].hi, &w[1].lo) {
                (Some(h), Some(l)) if h < l => {
                    ex.push(sco029(format!("gap between {} and {}", w[0].letter, w[1].letter)))
                }
                (Some(h), Some(l)) if h > l => ex.push(sco029(format!(
                    "overlap between {} and {}",
                    w[0].letter, w[1].letter
                ))),
                (Some(_), Some(_)) => {}
                _ => ex.push(sco029(format!(
                    "open boundary between {} and {}",
                    w[0].letter, w[1].letter
                ))),
            }
        }
        ex
    }

    /// The band containing `x` (exact comparison; boundaries belong to the upper band).
    pub fn band_for(&self, x: &Exact) -> Option<&GradeBand> {
        use std::cmp::Ordering::Less;
        self.bands.iter().find(|b| {
            b.lo.as_ref().is_none_or(|lo| cmp_exact(x, lo) != Less)
                && b.hi.as_ref().is_none_or(|hi| cmp_exact(x, hi) == Less)
        })
    }

    /// Lower bounds of every band except the first: the grade boundaries b (§10.2).
    pub fn boundaries(&self) -> Vec<Exact> {
        self.bands.iter().filter_map(|b| b.lo).collect()
    }
}

/// L6 status codes that replace the letter (§10.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StatusCode {
    /// Denied entry (attendance < 75 %): no final sitting, counted as F.
    DN,
    /// In progress (course spans terms).
    IP,
    /// Incomplete.
    IC,
    /// No-grade pass (≥ 60).
    NP,
    /// No-grade fail (< 60).
    NF,
    /// Withdrawn.
    W,
    /// Exempted (meaning unverified, §10.6).
    E,
}

/// TP-154 A.1 status columns (§11.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum A1Column {
    /// Denied Entry.
    DeniedEntry,
    /// In Progress.
    InProgress,
    /// Incomplete.
    Incomplete,
    /// Pass.
    Pass,
    /// Fail.
    Fail,
    /// Withdrawn.
    Withdrawn,
}

impl StatusCode {
    /// The code as printed.
    pub fn code(self) -> &'static str {
        match self {
            StatusCode::DN => "DN",
            StatusCode::IP => "IP",
            StatusCode::IC => "IC",
            StatusCode::NP => "NP",
            StatusCode::NF => "NF",
            StatusCode::W => "W",
            StatusCode::E => "E",
        }
    }

    /// Arabic label (report-template package wording may override).
    pub fn arabic(self) -> &'static str {
        match self {
            StatusCode::DN => "محروم",
            StatusCode::IP => "مستمر",
            StatusCode::IC => "غير مكتمل",
            StatusCode::NP => "ناجح",
            StatusCode::NF => "راسب",
            StatusCode::W => "منسحب",
            StatusCode::E => "معفى",
        }
    }

    /// The TP-154 A.1 column (E maps to none, pending the package mapping).
    pub fn a1_column(self) -> Option<A1Column> {
        match self {
            StatusCode::DN => Some(A1Column::DeniedEntry),
            StatusCode::IP => Some(A1Column::InProgress),
            StatusCode::IC => Some(A1Column::Incomplete),
            StatusCode::NP => Some(A1Column::Pass),
            StatusCode::NF => Some(A1Column::Fail),
            StatusCode::W => Some(A1Column::Withdrawn),
            StatusCode::E => None,
        }
    }

    /// GPA points: DN counts as F (1.00 / 0.00); the others carry none.
    pub fn gpa(self) -> Option<(Points, Points)> {
        match self {
            StatusCode::DN => Some((pts(1, 1), pts(0, 1))),
            _ => None,
        }
    }
}

/// An L6 outcome: a letter from the scale, or a status code.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GradeOutcome {
    /// A letter grade.
    Letter {
        /// English letter.
        letter: String,
        /// Arabic letter.
        arabic: String,
        /// 5-point GPA.
        gpa5: Points,
        /// 4-point GPA.
        gpa4: Points,
    },
    /// A status code.
    Status(StatusCode),
}

impl GradeOutcome {
    /// Letter outcome for `x` on `scale` (`None` only if the scale is invalid).
    pub fn letter_for(scale: &GradeScale, x: &Exact) -> Option<Self> {
        scale.band_for(x).map(|b| GradeOutcome::Letter {
            letter: b.letter.clone(),
            arabic: b.arabic.clone(),
            gpa5: b.gpa5,
            gpa4: b.gpa4,
        })
    }

    /// Display code: letter or status code.
    pub fn code(&self) -> &str {
        match self {
            GradeOutcome::Letter { letter, .. } => letter,
            GradeOutcome::Status(s) => s.code(),
        }
    }

    /// 5-point GPA (DN as F), `None` for status codes without GPA.
    pub fn gpa5(&self) -> Option<Points> {
        match self {
            GradeOutcome::Letter { gpa5, .. } => Some(*gpa5),
            GradeOutcome::Status(s) => s.gpa().map(|g| g.0),
        }
    }

    /// 4-point GPA (DN as F), `None` for status codes without GPA.
    pub fn gpa4(&self) -> Option<Points> {
        match self {
            GradeOutcome::Letter { gpa4, .. } => Some(*gpa4),
            GradeOutcome::Status(s) => s.gpa().map(|g| g.1),
        }
    }

    /// True for a passing letter (not F) or NP. DN, NF and other statuses are not passing.
    pub fn is_passing(&self) -> bool {
        match self {
            GradeOutcome::Letter { letter, .. } => letter != "F",
            GradeOutcome::Status(s) => *s == StatusCode::NP,
        }
    }
}
