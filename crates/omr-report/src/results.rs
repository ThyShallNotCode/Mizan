//! Results export: one row per student with responses, scores and the L6 grade (Saudi letter
//! and Arabic letter, 5- and 4-point GPA) (spec §17.2 "Grade list", §10.6, §16.3).
//!
//! Responses are master-space positional codes (§20.2): `A`–`Z` for options 1–26, several
//! letters for a multi-mark or a multi-select set, empty for a blank or an item not on the
//! student's form, `?` for a pending or unreadable response, and the exact points of a score
//! box. The label set is stated in the workbook's "About" sheet.

use crate::error::ReportError;
use crate::meta::{Language, ReportMeta};
use crate::table::{Cell, Row, Sheet, write_csv, write_xlsx};
use crate::text::{exact_str, mask_code};
use omr_model::StudentKey;
use omr_scoring::response::Cell as MCell;
use omr_scoring::{
    AssessmentStatus, Exact, GradeOutcome, GradeScale, ResponseMatrix, RoundingPolicy, ScoreRun,
    widen,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;

/// Display data for a student (PII: name and SIS ID come from the vault at report time).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudentInfo {
    /// SIS student ID as text (defaults to the student key).
    pub sis_id: Option<String>,
    /// Display name (Arabic or Latin, never mutated).
    pub name: Option<String>,
    /// Section.
    pub section: Option<String>,
}

/// One student row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultRow {
    /// SIS student ID (text).
    pub student_id: Option<String>,
    /// Name.
    pub name: Option<String>,
    /// Section.
    pub section: Option<String>,
    /// Booklet serial.
    pub booklet: u32,
    /// Form letter.
    pub form: String,
    /// Status code: `scored`, `provisional`, `not_scorable`, `absent_excused`, `absent`.
    pub status: String,
    /// Raw points (counted, after floors).
    pub raw: Option<Exact>,
    /// Maximum (counted non-bonus).
    pub max: Option<Exact>,
    /// Released percentage (L4 step 8).
    pub percent: Option<Exact>,
    /// Letter (A+ … F) or a status code.
    pub letter: Option<String>,
    /// Arabic letter (أ+ … هـ) or the Arabic status label.
    pub letter_ar: Option<String>,
    /// 5-point GPA.
    pub gpa5: Option<Exact>,
    /// 4-point GPA.
    pub gpa4: Option<Exact>,
    /// Passing.
    pub passing: Option<bool>,
    /// Responses per master item (positional codes).
    pub responses: Vec<String>,
}

/// The results table of one score run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultsTable {
    /// Header and provenance.
    pub meta: ReportMeta,
    /// Master item labels.
    pub items: Vec<String>,
    /// Decimal places of displayed percentages (the rounding policy's).
    pub decimals: u32,
    /// Grade scale name.
    pub scale: String,
    /// Rows in score-run order.
    pub rows: Vec<ResultRow>,
}

fn status_code(s: AssessmentStatus) -> &'static str {
    match s {
        AssessmentStatus::Scored => "scored",
        AssessmentStatus::Provisional => "provisional",
        AssessmentStatus::NotScorable => "not_scorable",
        AssessmentStatus::Absent { excused: true } => "absent_excused",
        AssessmentStatus::Absent { excused: false } => "absent",
    }
}

fn response_code(c: &MCell) -> String {
    match c {
        MCell::Answered(m) => mask_code(*m),
        MCell::Blank | MCell::NotPresented | MCell::NotApplicable => String::new(),
        MCell::Pending => "?".into(),
        MCell::Points(p) => exact_str(&widen(p), 2),
    }
}

impl ResultsTable {
    /// Builds the table from a score run, its L2 matrix (for the responses), the grade scale
    /// and rounding policy (for the letters) and the students' display data.
    pub fn from_run(
        meta: ReportMeta,
        run: &ScoreRun,
        matrix: &ResponseMatrix,
        scale: &GradeScale,
        rounding: &RoundingPolicy,
        students: &BTreeMap<StudentKey, StudentInfo>,
    ) -> Result<Self, ReportError> {
        if matrix.n_rows != run.n_rows() || matrix.n_items != run.items.len() {
            return Err(ReportError::Invalid(
                "response matrix does not match the score run".into(),
            ));
        }
        let grades = run.grades(scale, rounding);
        let rows = run
            .students
            .iter()
            .enumerate()
            .map(|(n, s)| {
                let info = s.student.as_ref().and_then(|k| students.get(k));
                let grade = grades[n].as_ref();
                let scored = !matches!(s.status, AssessmentStatus::Absent { .. })
                    && s.status != AssessmentStatus::NotScorable;
                ResultRow {
                    student_id: info
                        .and_then(|i| i.sis_id.clone())
                        .or_else(|| s.student.as_ref().map(|k| k.0.clone())),
                    name: info.and_then(|i| i.name.clone()),
                    section: info.and_then(|i| i.section.clone()),
                    booklet: s.booklet.0,
                    form: s.form.letter().to_string(),
                    status: status_code(s.status).into(),
                    raw: scored.then_some(s.raw),
                    max: scored.then_some(s.max),
                    percent: s.final_pct.map(|x| rounding.round(&x)),
                    letter: grade.map(|g| g.code().to_owned()),
                    letter_ar: grade.map(|g| match g {
                        GradeOutcome::Letter { arabic, .. } => arabic.clone(),
                        GradeOutcome::Status(st) => st.arabic().to_owned(),
                    }),
                    gpa5: grade.and_then(|g| g.gpa5()).map(|p| widen(&p)),
                    gpa4: grade.and_then(|g| g.gpa4()).map(|p| widen(&p)),
                    passing: grade.map(GradeOutcome::is_passing),
                    responses: (0..matrix.n_items)
                        .map(|i| response_code(&matrix.cell(n, i)))
                        .collect(),
                }
            })
            .collect();
        Ok(Self {
            meta,
            items: run.items.iter().map(|m| m.label.clone()).collect(),
            decimals: rounding.decimals,
            scale: scale.name.clone(),
            rows,
        })
    }

    /// Column headers in the report language.
    pub fn headers(&self) -> Vec<String> {
        let fixed: [&str; 13] = match self.meta.language {
            Language::En => [
                "Student ID",
                "Name",
                "Section",
                "Booklet",
                "Form",
                "Status",
                "Raw score",
                "Maximum",
                "Percent",
                "Letter",
                "Letter (Arabic)",
                "GPA (5)",
                "GPA (4)",
            ],
            Language::Ar => [
                "الرقم الجامعي",
                "الاسم",
                "الشعبة",
                "رقم الكراسة",
                "النموذج",
                "الحالة",
                "الدرجة الخام",
                "الدرجة العظمى",
                "النسبة المئوية",
                "التقدير",
                "التقدير بالعربية",
                "المعدل من 5",
                "المعدل من 4",
            ],
        };
        fixed
            .iter()
            .map(|s| (*s).to_owned())
            .chain(self.items.iter().cloned())
            .collect()
    }

    /// The results sheet.
    pub fn sheet(&self) -> Sheet {
        let d = self.decimals;
        let rows = self
            .rows
            .iter()
            .map(|r| {
                let mut cells = vec![
                    r.student_id.clone().map_or(Cell::Empty, Cell::Id),
                    Cell::opt_text(r.name.clone()),
                    r.section.clone().map_or(Cell::Empty, Cell::Id),
                    Cell::Int(i64::from(r.booklet)),
                    Cell::text(r.form.clone()),
                    Cell::text(r.status.clone()),
                    Cell::num(r.raw.as_ref().map(|x| exact_str(x, 2)), 2),
                    Cell::num(r.max.as_ref().map(|x| exact_str(x, 2)), 2),
                    Cell::num(r.percent.as_ref().map(|x| exact_str(x, d)), d),
                    Cell::opt_text(r.letter.clone()),
                    Cell::opt_text(r.letter_ar.clone()),
                    Cell::num(r.gpa5.as_ref().map(|x| exact_str(x, 2)), 2),
                    Cell::num(r.gpa4.as_ref().map(|x| exact_str(x, 2)), 2),
                ];
                cells.extend(r.responses.iter().map(|s| {
                    if s.is_empty() {
                        Cell::Empty
                    } else {
                        Cell::text(s.clone())
                    }
                }));
                Row::new(cells)
            })
            .collect();
        let mut widths = vec![
            14.0, 28.0, 9.0, 9.0, 6.0, 13.0, 10.0, 9.0, 9.0, 7.0, 9.0, 8.0, 8.0,
        ];
        widths.extend(std::iter::repeat_n(5.0, self.items.len()));
        Sheet {
            name: "Results".into(),
            title: self.meta.title.clone(),
            headers: vec![self.headers()],
            rows,
            notes: vec![],
            rtl: self.meta.language.rtl(),
            widths,
        }
    }

    /// The "About" sheet: label set, codes, scale and provenance.
    pub fn about_sheet(&self) -> Sheet {
        let mut rows = vec![
            ("Classification", self.meta.classification.label()),
            ("Grade scale", self.scale.clone()),
            (
                "Response codes",
                "A–Z = option 1–26 (positional); several letters = multi-mark or set; empty = \
                 blank or not on the form; ? = pending or unreadable; numbers = score-box points"
                    .into(),
            ),
            (
                "Status codes",
                "scored, provisional, not_scorable, absent_excused, absent".into(),
            ),
        ];
        rows.extend(
            self.meta
                .provenance()
                .into_iter()
                .filter(|(k, _)| *k != "Classification"),
        );
        Sheet {
            name: "About".into(),
            title: format!("{} — about this export", self.meta.title),
            headers: vec![vec!["Field".into(), "Value".into()]],
            rows: rows
                .into_iter()
                .map(|(k, v)| Row::new(vec![Cell::text(k), Cell::text(v)]))
                .collect(),
            notes: vec![],
            rtl: false,
            widths: vec![20.0, 100.0],
        }
    }

    /// Writes the results as CSV (data rows only; the label goes to the export manifest).
    pub fn write_csv<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_csv(w, &self.sheet())
    }

    /// Writes the results as XLSX (Results + About sheets).
    pub fn write_xlsx<W: Write>(&self, w: &mut W) -> Result<(), ReportError> {
        write_xlsx(w, &self.meta, &[self.sheet(), self.about_sheet()])
    }
}
