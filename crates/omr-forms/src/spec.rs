//! The template specification: what an exam office asks for (spec §6.1, §6.2 wizard, §6.5).
//!
//! A [`TemplateSpec`] is validated and laid out by [`crate::Template::build`]. Display text
//! (title, course, instructions) is carried here but is **not** part of the template hash (§6.1).

use omr_model::{CancelMode, ExamId, TemplateRevision};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Smallest number of options per item (§6.5: 2–26 bubbles; the MVP caps at 10).
pub const MIN_OPTIONS: u8 = 2;
/// Largest number of options per item in the MVP layout (one row, no wrapping).
pub const MAX_OPTIONS: u8 = 10;
/// Largest number of questions per template.
pub const MAX_QUESTIONS: u16 = 1_000;
/// Largest number of forms (variants), so every form has a letter A–Z.
pub const MAX_FORMS: u16 = 26;
/// Largest number of student-ID digits.
pub const MAX_ID_DIGITS: u8 = 12;
/// Largest revision encodable in the page code (10 bits, §6.4).
pub const MAX_REVISION: u16 = 1_023;

/// Paper size. The MVP lays out A4 only (Letter has a different safe box, §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Paper {
    /// ISO A4, 210 × 297 mm.
    #[default]
    A4,
}

/// Option label set (§6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum LabelSet {
    /// A, B, C … J.
    #[default]
    Latin,
    /// أ ب ج د هـ و ز ح ط ي (abjadī order); rows run right to left.
    Arabic,
    /// 1, 2, 3 … 10.
    Numeric,
}

impl LabelSet {
    /// The printed label of option `i` (0-based), for `i < MAX_OPTIONS`.
    pub fn label(self, i: u8) -> &'static str {
        const LATIN: [&str; 10] = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
        const ARABIC: [&str; 10] = ["أ", "ب", "ج", "د", "هـ", "و", "ز", "ح", "ط", "ي"];
        const NUMERIC: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        let i = usize::from(i.min(MAX_OPTIONS - 1));
        match self {
            LabelSet::Latin => LATIN[i],
            LabelSet::Arabic => ARABIC[i],
            LabelSet::Numeric => NUMERIC[i],
        }
    }

    /// The natural row direction for this label set.
    pub fn direction(self) -> Direction {
        match self {
            LabelSet::Arabic => Direction::Rtl,
            _ => Direction::Ltr,
        }
    }
}

/// Labels printed inside ID-grid bubbles (§6.5: optional ٠–٩, stored as ASCII).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DigitLabels {
    /// 0–9.
    #[default]
    Western,
    /// ٠–٩ (Arabic-Indic).
    ArabicIndic,
}

impl DigitLabels {
    /// Printed label of digit `d` (0–9).
    pub fn label(self, d: u8) -> &'static str {
        const W: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
        const A: [&str; 10] = ["٠", "١", "٢", "٣", "٤", "٥", "٦", "٧", "٨", "٩"];
        let d = usize::from(d.min(9));
        match self {
            DigitLabels::Western => W[d],
            DigitLabels::ArabicIndic => A[d],
        }
    }
}

/// Reading direction of answer rows and columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Direction {
    /// Question number on the left, option 0 leftmost, first column leftmost.
    #[default]
    Ltr,
    /// Question number on the right, option 0 rightmost, first column rightmost.
    Rtl,
}

/// Which diagonal carries the two page-code copies (§6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CodeDiagonal {
    /// Top-right and bottom-left (default; staples at top-left).
    #[default]
    TrBl,
    /// Top-left and bottom-right (right-bound RTL booklets).
    TlBr,
}

/// Options per item: uniform or per question.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionCounts {
    /// Every item has this many options.
    Uniform(u8),
    /// Item `i` has `v[i]` options (`v.len()` must equal the question count).
    PerItem(Vec<u8>),
}

impl OptionCounts {
    /// Options of question `q` (0-based).
    pub fn of(&self, q: u16) -> u8 {
        match self {
            OptionCounts::Uniform(n) => *n,
            OptionCounts::PerItem(v) => v.get(usize::from(q)).copied().unwrap_or(0),
        }
    }

    /// The largest option count.
    pub fn max(&self) -> u8 {
        match self {
            OptionCounts::Uniform(n) => *n,
            OptionCounts::PerItem(v) => v.iter().copied().max().unwrap_or(0),
        }
    }
}

/// What an exam office asks for (§6.2 "New exam" wizard).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemplateSpec {
    /// Exam title, printed in the header (not hashed).
    pub title: String,
    /// Course code and name, printed in the header and the echo line (not hashed).
    pub course: String,
    /// The exam this template belongs to.
    pub exam: ExamId,
    /// Collision salt for the 40-bit exam ID (§6.4); incremented by the registry on collision.
    #[serde(default)]
    pub exam_salt: u8,
    /// Template revision (10 bits in the page code).
    pub revision: TemplateRevision,
    /// Paper size.
    #[serde(default)]
    pub paper: Paper,
    /// Number of questions (1–1,000).
    pub questions: u16,
    /// Options per question (2–10).
    pub options: OptionCounts,
    /// Option label set.
    #[serde(default)]
    pub labels: LabelSet,
    /// Number of forms (variants), 1–26; form codes are 1 = A, 2 = B, …
    pub forms: u16,
    /// Student-ID digits (0 disables the ID grid).
    pub id_digits: u8,
    /// Labels inside ID-grid bubbles.
    #[serde(default)]
    pub id_digit_labels: DigitLabels,
    /// Print a bubbled form/version grid on page 1 (§6.5 admin bubbles).
    #[serde(default)]
    pub form_grid: bool,
    /// Marking convention. **Mandatory, no default** (§6.5, lint L14): `None` blocks the build.
    pub cancel_mode: Option<CancelMode>,
    /// Extra instructions printed in the header (not hashed).
    #[serde(default)]
    pub instructions: String,
    /// Which diagonal carries the page codes.
    #[serde(default)]
    pub code_diagonal: CodeDiagonal,
    /// Row direction; `None` follows the label set (Arabic → right to left).
    #[serde(default)]
    pub direction: Option<Direction>,
}

impl TemplateSpec {
    /// A common-case spec (§2.4 standard slice): Latin labels, one form, a 9-digit ID grid.
    pub fn standard(exam: ExamId, questions: u16, options: u8, cancel_mode: CancelMode) -> Self {
        Self {
            title: "Midterm Examination".into(),
            course: "COURSE 101".into(),
            exam,
            exam_salt: 0,
            revision: TemplateRevision(1),
            paper: Paper::A4,
            questions,
            options: OptionCounts::Uniform(options),
            labels: LabelSet::Latin,
            forms: 1,
            id_digits: 9,
            id_digit_labels: DigitLabels::Western,
            form_grid: false,
            cancel_mode: Some(cancel_mode),
            instructions: String::new(),
            code_diagonal: CodeDiagonal::TrBl,
            direction: None,
        }
    }

    /// The effective row direction.
    pub fn direction(&self) -> Direction {
        self.direction.unwrap_or_else(|| self.labels.direction())
    }

    /// Checks ranges and consistency (lint L14 is reported by the build, not here).
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.questions == 0 || self.questions > MAX_QUESTIONS {
            return Err(SpecError::Questions(self.questions));
        }
        if let OptionCounts::PerItem(v) = &self.options
            && v.len() != usize::from(self.questions)
        {
            return Err(SpecError::PerItemLength {
                expected: self.questions,
                got: v.len(),
            });
        }
        for q in 0..self.questions {
            let n = self.options.of(q);
            if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&n) {
                return Err(SpecError::Options { question: q, n });
            }
        }
        if self.forms == 0 || self.forms > MAX_FORMS {
            return Err(SpecError::Forms(self.forms));
        }
        if self.id_digits > MAX_ID_DIGITS {
            return Err(SpecError::IdDigits(self.id_digits));
        }
        if self.revision.0 > MAX_REVISION {
            return Err(SpecError::Revision(self.revision.0));
        }
        Ok(())
    }
}

/// A specification value out of range.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpecError {
    /// Question count outside 1–1,000.
    #[error("question count {0} is outside 1–{MAX_QUESTIONS}")]
    Questions(u16),
    /// Options outside 2–10.
    #[error("question {question} has {n} options; allowed {MIN_OPTIONS}–{MAX_OPTIONS}")]
    Options {
        /// 0-based question.
        question: u16,
        /// Option count.
        n: u8,
    },
    /// Per-item option list of the wrong length.
    #[error("per-item options list has {got} entries for {expected} questions")]
    PerItemLength {
        /// Question count.
        expected: u16,
        /// List length.
        got: usize,
    },
    /// Forms outside 1–26.
    #[error("form count {0} is outside 1–{MAX_FORMS}")]
    Forms(u16),
    /// Too many ID digits.
    #[error("{0} ID digits exceed the maximum of {MAX_ID_DIGITS}")]
    IdDigits(u8),
    /// Revision not encodable in 10 bits.
    #[error("revision {0} exceeds {MAX_REVISION}")]
    Revision(u16),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exam() -> ExamId {
        ExamId(uuid::Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0001,
        ))
    }

    #[test]
    fn standard_spec_validates() {
        let s = TemplateSpec::standard(exam(), 60, 5, CancelMode::EraseOnly);
        assert_eq!(s.validate(), Ok(()));
        assert_eq!(s.direction(), Direction::Ltr);
    }

    #[test]
    fn ranges_are_enforced() {
        let mut s = TemplateSpec::standard(exam(), 60, 5, CancelMode::EraseOnly);
        s.options = OptionCounts::Uniform(11);
        assert!(matches!(s.validate(), Err(SpecError::Options { .. })));
        s.options = OptionCounts::PerItem(vec![4; 59]);
        assert!(matches!(s.validate(), Err(SpecError::PerItemLength { .. })));
        s.options = OptionCounts::Uniform(4);
        s.forms = 0;
        assert_eq!(s.validate(), Err(SpecError::Forms(0)));
        s.forms = 2;
        s.revision = TemplateRevision(1024);
        assert_eq!(s.validate(), Err(SpecError::Revision(1024)));
    }

    #[test]
    fn labels() {
        assert_eq!(LabelSet::Latin.label(4), "E");
        assert_eq!(LabelSet::Arabic.label(0), "أ");
        assert_eq!(LabelSet::Arabic.direction(), Direction::Rtl);
        assert_eq!(DigitLabels::ArabicIndic.label(7), "٧");
    }
}
