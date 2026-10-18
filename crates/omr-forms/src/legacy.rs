//! Legacy and third-party form templates (spec §6.10, path (b) result): a bubble lattice in the
//! pixel frame of a blank reference image, for printed forms without fiducials (the IAU CCSIT
//! sheet). Engine code: parses bytes, never opens files (R4).
//!
//! The JSON format (see `templates/iau-ccsit-v1/template.json`):
//!
//! ```text
//! { "id", "name", "provenance",
//!   "reference": { "image", "width_px", "height_px", "dpi" },      // frame of the blank form
//!   "bubble_radius_px": 13.5,
//!   "answers": { "questions": 100, "options": ["A".."E"], "bubbles": [{ "q", "opt", "x", "y" }] },
//!   "student_number" | "section": { "digits": n, "bubbles": [{ "col", "digit", "x", "y" }] },
//!   "gender" | "version" | "exam": { "labels": [..], "bubbles": [{ "value", "x", "y" }] },
//!   "date": { "columns": ["D1", ..], "bubbles": [{ "col": "D1", "digit", "x", "y" }] } }
//! ```
//!
//! Coordinates are pixel-corner coordinates (pixel `(i, j)` covers `[i, i+1) × [j, j+1)`) at
//! `reference.dpi`. [`LegacyTemplate::bubbles`] flattens every bubble in a fixed order: answers
//! (question, option), student number, section, date (column, digit), gender, version, exam.

use crate::hash::{hex, sha384};
use omr_model::PointPx;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use thiserror::Error;

/// The committed IAU CCSIT template JSON (layout only, no student data).
pub const IAU_CCSIT_V1_JSON: &[u8] = include_bytes!("../templates/iau-ccsit-v1/template.json");
/// The committed blank IAU CCSIT form at 200 dpi (8-bit gray PNG), the template's reference frame.
pub const IAU_CCSIT_V1_BLANK_PNG: &[u8] =
    include_bytes!("../templates/iau-ccsit-v1/blank-200dpi.png");

/// Why a legacy template was rejected.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LegacyError {
    /// The JSON does not parse into the template format.
    #[error("template JSON: {0}")]
    Json(String),
    /// The template is structurally invalid.
    #[error("invalid template: {0}")]
    Invalid(String),
}

/// The reference frame (blank form image).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceFrame {
    /// File name of the blank form image (informational; bytes are passed separately).
    pub image: String,
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
    /// Resolution in dots per inch.
    pub dpi: f64,
}

/// One answer bubble as stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerBubbleSpec {
    /// 1-based question number.
    pub q: u16,
    /// Option label.
    pub opt: String,
    /// Centre x (px).
    pub x: f64,
    /// Centre y (px).
    pub y: f64,
}

/// The answer block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerBlockSpec {
    /// Number of questions printed.
    pub questions: u16,
    /// Option labels in printed order.
    pub options: Vec<String>,
    /// Every answer bubble.
    pub bubbles: Vec<AnswerBubbleSpec>,
}

/// One digit-grid bubble with a numeric column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigitBubbleSpec {
    /// 0-based column (0 = leftmost).
    pub col: u8,
    /// Digit value.
    pub digit: u8,
    /// Centre x (px).
    pub x: f64,
    /// Centre y (px).
    pub y: f64,
}

/// A bubbled digit grid (student number, section).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigitGridSpec {
    /// Number of columns.
    pub digits: u8,
    /// Every bubble.
    pub bubbles: Vec<DigitBubbleSpec>,
}

/// One choice bubble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceBubbleSpec {
    /// Its label.
    pub value: String,
    /// Centre x (px).
    pub x: f64,
    /// Centre y (px).
    pub y: f64,
}

/// A single-choice field (gender, version, exam type).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceFieldSpec {
    /// Labels in printed order.
    pub labels: Vec<String>,
    /// Every bubble.
    pub bubbles: Vec<ChoiceBubbleSpec>,
}

/// One date bubble (named column).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DateBubbleSpec {
    /// Column name (e.g. `D1`).
    pub col: String,
    /// Digit value.
    pub digit: u8,
    /// Centre x (px).
    pub x: f64,
    /// Centre y (px).
    pub y: f64,
}

/// The date grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DateGridSpec {
    /// Column names in printed order.
    pub columns: Vec<String>,
    /// Every bubble.
    pub bubbles: Vec<DateBubbleSpec>,
}

/// The template as stored in JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyTemplateSpec {
    /// Stable identifier, e.g. `iau-ccsit-v1`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// How the lattice was obtained.
    #[serde(default)]
    pub provenance: String,
    /// Reference frame.
    pub reference: ReferenceFrame,
    /// Nominal printed bubble radius in reference pixels.
    pub bubble_radius_px: f64,
    /// Answer block.
    pub answers: AnswerBlockSpec,
    /// Student-number grid.
    pub student_number: DigitGridSpec,
    /// Section grid.
    pub section: DigitGridSpec,
    /// Gender choice.
    pub gender: ChoiceFieldSpec,
    /// Version (form) choice.
    pub version: ChoiceFieldSpec,
    /// Exam-type choice.
    pub exam: ChoiceFieldSpec,
    /// Date grid.
    pub date: DateGridSpec,
}

/// The field a bubble belongs to, in flattening order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LegacyField {
    /// Answer bubbles (group = 0-based question).
    Answers,
    /// Student-number grid (group = column).
    StudentNumber,
    /// Section grid (group = column).
    Section,
    /// Date grid (group = column index into `date.columns`).
    Date,
    /// Gender (group 0).
    Gender,
    /// Version (group 0).
    Version,
    /// Exam type (group 0).
    Exam,
}

impl LegacyField {
    /// All fields in flattening order.
    pub const ALL: [LegacyField; 7] = [
        LegacyField::Answers,
        LegacyField::StudentNumber,
        LegacyField::Section,
        LegacyField::Date,
        LegacyField::Gender,
        LegacyField::Version,
        LegacyField::Exam,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// One bubble of the flattened template.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyBubble {
    /// Field.
    pub field: LegacyField,
    /// Question (0-based) or column index.
    pub group: u16,
    /// Option index within the group (answer option, digit value, or choice label index).
    pub option: u8,
    /// Centre in reference pixels.
    pub centre: PointPx,
}

/// A validated legacy template with typed accessors.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyTemplate {
    spec: LegacyTemplateSpec,
    bubbles: Vec<LegacyBubble>,
    field_ranges: [Range<usize>; 7],
    groups: [Vec<Range<usize>>; 7],
    json_sha384: [u8; 48],
}

fn invalid(msg: impl Into<String>) -> LegacyError {
    LegacyError::Invalid(msg.into())
}

impl LegacyTemplate {
    /// Parses and validates template JSON bytes.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LegacyError> {
        let spec: LegacyTemplateSpec =
            serde_json::from_slice(bytes).map_err(|e| LegacyError::Json(e.to_string()))?;
        let mut t = Self::from_spec(spec)?;
        t.json_sha384 = sha384(bytes);
        Ok(t)
    }

    /// The committed IAU CCSIT template (`templates/iau-ccsit-v1`).
    pub fn iau_ccsit_v1() -> Self {
        Self::from_json(IAU_CCSIT_V1_JSON).expect("the committed IAU template is valid")
    }

    /// Validates a parsed spec (the hash is of its canonical re-serialisation).
    pub fn from_spec(spec: LegacyTemplateSpec) -> Result<Self, LegacyError> {
        let r = &spec.reference;
        if r.width_px == 0 || r.height_px == 0 || !r.dpi.is_finite() || r.dpi <= 0.0 {
            return Err(invalid("reference frame must have positive size and dpi"));
        }
        if !spec.bubble_radius_px.is_finite() || spec.bubble_radius_px <= 0.0 {
            return Err(invalid("bubble_radius_px must be positive"));
        }
        let (w, h) = (f64::from(r.width_px), f64::from(r.height_px));
        let check_xy = |x: f64, y: f64, what: &str| {
            if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < w && y < h {
                Ok(())
            } else {
                Err(invalid(format!(
                    "{what}: centre ({x}, {y}) outside the frame"
                )))
            }
        };
        let mut bubbles = Vec::new();
        let mut field_ranges: [Range<usize>; 7] = Default::default();
        let mut groups: [Vec<Range<usize>>; 7] = Default::default();
        let push_group = |field: LegacyField,
                          group: u16,
                          mut items: Vec<(u8, f64, f64)>,
                          expected: Option<usize>,
                          bubbles: &mut Vec<LegacyBubble>|
         -> Result<Range<usize>, LegacyError> {
            items.sort_by_key(|i| i.0);
            for (k, it) in items.iter().enumerate() {
                if usize::from(it.0) != k {
                    return Err(invalid(format!(
                        "{field:?} group {group}: options must be 0..n without gaps or repeats"
                    )));
                }
            }
            if items.len() < 2 || expected.is_some_and(|e| e != items.len()) {
                return Err(invalid(format!(
                    "{field:?} group {group}: has {} bubbles",
                    items.len()
                )));
            }
            let start = bubbles.len();
            for (o, x, y) in items {
                check_xy(x, y, &format!("{field:?} group {group}"))?;
                bubbles.push(LegacyBubble {
                    field,
                    group,
                    option: o,
                    centre: PointPx { x, y },
                });
            }
            Ok(start..bubbles.len())
        };

        // Answers.
        let a = &spec.answers;
        if a.questions == 0 || a.options.is_empty() || a.options.len() > 26 {
            return Err(invalid("answers: need ≥ 1 question and 1–26 options"));
        }
        let mut per_q: Vec<Vec<(u8, f64, f64)>> = vec![Vec::new(); usize::from(a.questions)];
        for b in &a.bubbles {
            if b.q == 0 || b.q > a.questions {
                return Err(invalid(format!("answers: question {} out of range", b.q)));
            }
            let o = a
                .options
                .iter()
                .position(|o| *o == b.opt)
                .ok_or_else(|| invalid(format!("answers: unknown option {}", b.opt)))?;
            per_q[usize::from(b.q - 1)].push((o as u8, b.x, b.y));
        }
        let start = bubbles.len();
        for (q, items) in per_q.into_iter().enumerate() {
            let r = push_group(
                LegacyField::Answers,
                q as u16,
                items,
                Some(a.options.len()),
                &mut bubbles,
            )?;
            groups[LegacyField::Answers.index()].push(r);
        }
        field_ranges[LegacyField::Answers.index()] = start..bubbles.len();

        // Digit grids.
        for (field, g) in [
            (LegacyField::StudentNumber, &spec.student_number),
            (LegacyField::Section, &spec.section),
        ] {
            let mut cols: Vec<Vec<(u8, f64, f64)>> = vec![Vec::new(); usize::from(g.digits)];
            for b in &g.bubbles {
                let c = cols
                    .get_mut(usize::from(b.col))
                    .ok_or_else(|| invalid(format!("{field:?}: column {} out of range", b.col)))?;
                c.push((b.digit, b.x, b.y));
            }
            let start = bubbles.len();
            for (c, items) in cols.into_iter().enumerate() {
                let r = push_group(field, c as u16, items, Some(10), &mut bubbles)?;
                groups[field.index()].push(r);
            }
            field_ranges[field.index()] = start..bubbles.len();
        }

        // Date.
        let d = &spec.date;
        let mut cols: Vec<Vec<(u8, f64, f64)>> = vec![Vec::new(); d.columns.len()];
        for b in &d.bubbles {
            let c = d
                .columns
                .iter()
                .position(|c| *c == b.col)
                .ok_or_else(|| invalid(format!("date: unknown column {}", b.col)))?;
            cols[c].push((b.digit, b.x, b.y));
        }
        let start = bubbles.len();
        for (c, items) in cols.into_iter().enumerate() {
            let r = push_group(LegacyField::Date, c as u16, items, None, &mut bubbles)?;
            groups[LegacyField::Date.index()].push(r);
        }
        field_ranges[LegacyField::Date.index()] = start..bubbles.len();

        // Choices.
        for (field, g) in [
            (LegacyField::Gender, &spec.gender),
            (LegacyField::Version, &spec.version),
            (LegacyField::Exam, &spec.exam),
        ] {
            let mut items = Vec::new();
            for b in &g.bubbles {
                let o = g
                    .labels
                    .iter()
                    .position(|l| *l == b.value)
                    .ok_or_else(|| invalid(format!("{field:?}: unknown label {}", b.value)))?;
                items.push((o as u8, b.x, b.y));
            }
            let start = bubbles.len();
            let r = push_group(field, 0, items, Some(g.labels.len()), &mut bubbles)?;
            groups[field.index()].push(r);
            field_ranges[field.index()] = start..bubbles.len();
        }

        let canonical = serde_json::to_vec(&spec).map_err(|e| LegacyError::Json(e.to_string()))?;
        Ok(Self {
            spec,
            bubbles,
            field_ranges,
            groups,
            json_sha384: sha384(&canonical),
        })
    }

    /// The parsed JSON.
    pub fn spec(&self) -> &LegacyTemplateSpec {
        &self.spec
    }

    /// Template identifier.
    pub fn id(&self) -> &str {
        &self.spec.id
    }

    /// Reference frame size `(width, height)` in pixels.
    pub fn frame_size(&self) -> (u32, u32) {
        (self.spec.reference.width_px, self.spec.reference.height_px)
    }

    /// Reference resolution (dpi).
    pub fn dpi(&self) -> f64 {
        self.spec.reference.dpi
    }

    /// Nominal bubble radius in reference pixels.
    pub fn bubble_radius_px(&self) -> f64 {
        self.spec.bubble_radius_px
    }

    /// Every bubble in flattening order.
    pub fn bubbles(&self) -> &[LegacyBubble] {
        &self.bubbles
    }

    /// Index range of a field within [`Self::bubbles`].
    pub fn field_range(&self, field: LegacyField) -> Range<usize> {
        self.field_ranges[field.index()].clone()
    }

    /// Index ranges of a field's groups (questions or columns), in order.
    pub fn groups(&self, field: LegacyField) -> &[Range<usize>] {
        &self.groups[field.index()]
    }

    /// Number of printed questions.
    pub fn question_count(&self) -> u16 {
        self.spec.answers.questions
    }

    /// Answer option labels.
    pub fn option_labels(&self) -> &[String] {
        &self.spec.answers.options
    }

    /// The bubbles of question `q` (0-based), in option order.
    pub fn question(&self, q: u16) -> &[LegacyBubble] {
        &self.bubbles[self.groups[LegacyField::Answers.index()][usize::from(q)].clone()]
    }

    /// Labels of a choice field (`Gender`, `Version`, `Exam`); empty for other fields.
    pub fn choice_labels(&self, field: LegacyField) -> &[String] {
        match field {
            LegacyField::Gender => &self.spec.gender.labels,
            LegacyField::Version => &self.spec.version.labels,
            LegacyField::Exam => &self.spec.exam.labels,
            _ => &[],
        }
    }

    /// Date column names.
    pub fn date_columns(&self) -> &[String] {
        &self.spec.date.columns
    }

    /// SHA-384 (hex) of the template JSON as parsed (`from_json`) or re-serialised
    /// (`from_spec`).
    pub fn json_sha384_hex(&self) -> String {
        hex(&self.json_sha384)
    }

    /// SHA-384 (hex) of the template revision: the template JSON digest and the reference
    /// image bytes, domain-separated (for `EngineManifest::template_sha384`).
    pub fn revision_sha384_hex(&self, reference_image: &[u8]) -> String {
        let mut buf = Vec::with_capacity(64 + 48 + reference_image.len());
        buf.extend_from_slice(b"omr-legacy-template-v1\0");
        buf.extend_from_slice(&self.json_sha384);
        buf.extend_from_slice(&(reference_image.len() as u64).to_le_bytes());
        buf.extend_from_slice(reference_image);
        hex(&sha384(&buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iau_template_parses_with_687_bubbles() {
        let t = LegacyTemplate::iau_ccsit_v1();
        assert_eq!(t.bubbles().len(), 687);
        assert_eq!(t.question_count(), 100);
        assert_eq!(t.option_labels().len(), 5);
        assert_eq!(t.groups(LegacyField::Answers).len(), 100);
        assert_eq!(t.groups(LegacyField::StudentNumber).len(), 10);
        assert_eq!(t.groups(LegacyField::Section).len(), 3);
        assert_eq!(t.groups(LegacyField::Date).len(), 6);
        assert_eq!(t.field_range(LegacyField::Date).len(), 46);
        assert_eq!(t.choice_labels(LegacyField::Version), ["A", "B", "C", "D"]);
        assert_eq!(t.frame_size(), (2356, 1661));
        let q0 = t.question(0);
        assert!(q0.windows(2).all(|w| w[0].centre.x < w[1].centre.x));
        assert!(
            q0.iter()
                .enumerate()
                .all(|(i, b)| usize::from(b.option) == i)
        );
        // Order: answers, student number, section, date, gender, version, exam.
        let mut last = LegacyField::Answers;
        for b in t.bubbles() {
            assert!(b.field >= last);
            last = b.field;
        }
        assert_eq!(t.json_sha384_hex().len(), 96);
        assert_ne!(
            t.revision_sha384_hex(IAU_CCSIT_V1_BLANK_PNG),
            t.revision_sha384_hex(b"other")
        );
        assert_eq!(&IAU_CCSIT_V1_BLANK_PNG[1..4], b"PNG");
    }

    #[test]
    fn rejects_bad_templates() {
        let mut spec = LegacyTemplate::iau_ccsit_v1().spec().clone();
        spec.answers.bubbles.pop();
        assert!(matches!(
            LegacyTemplate::from_spec(spec),
            Err(LegacyError::Invalid(_))
        ));
        let mut spec = LegacyTemplate::iau_ccsit_v1().spec().clone();
        spec.section.bubbles[0].x = 1e9;
        assert!(LegacyTemplate::from_spec(spec).is_err());
        assert!(matches!(
            LegacyTemplate::from_json(b"{}"),
            Err(LegacyError::Json(_))
        ));
    }
}
