//! NCAAA TP-154 Course Report tables (spec §11.7): A.1 grade distribution and B.1 CLO
//! assessment results, as XLSX and CSV with English and Arabic headers.
//!
//! The rows come from `omr_outcomes::{tp154_a1, tp154_b1}`. The header wording is data
//! ([`Tp154Labels`]): the built-in defaults follow the spec's field names and ETEC's glossary
//! ("Course Learning Outcomes" = "نواتج تعلم المقرر", §20.1) and are replaced by the pinned,
//! signed report-template package (§11.7) when one is installed. Filling the official DOCX
//! blank is deferred (docs/IMPLEMENTATION.md §2).

use crate::error::ReportError;
use crate::meta::ReportMeta;
use crate::table::{Cell, Row, Sheet, write_csv, write_xlsx};
use crate::text::exact_str;
use omr_outcomes::{A1Table, B1Table};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Which header rows to print.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Headers {
    /// English row only.
    English,
    /// Arabic row only.
    Arabic,
    /// English row, then Arabic row (default).
    #[default]
    Both,
}

/// One language's TP-154 wording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tp154Text {
    /// A.1 title.
    pub a1_title: String,
    /// A.1 first header cell.
    pub grade: String,
    /// A.1 count row.
    pub number_of_students: String,
    /// A.1 percentage row.
    pub percentage: String,
    /// A.1 status columns: Denied Entry, In Progress, Incomplete, Pass, Fail, Withdrawn.
    pub statuses: [String; 6],
    /// B.1 title.
    pub b1_title: String,
    /// B.1 columns: #, CLO, related PLO code, assessment methods, targeted, actual, comment.
    pub b1_columns: [String; 7],
}

/// TP-154 wording in both languages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tp154Labels {
    /// English.
    pub en: Tp154Text,
    /// Arabic.
    pub ar: Tp154Text,
    /// Source of the wording (package ID and SHA-384, or "built-in").
    pub source: String,
}

fn s<const N: usize>(a: [&str; N]) -> [String; N] {
    a.map(str::to_owned)
}

impl Default for Tp154Labels {
    fn default() -> Self {
        Self {
            en: Tp154Text {
                a1_title: "A.1 Grade Distribution".into(),
                grade: "Grade".into(),
                number_of_students: "Number of Students".into(),
                percentage: "Percentage".into(),
                statuses: s([
                    "Denied Entry",
                    "In Progress",
                    "Incomplete",
                    "Pass",
                    "Fail",
                    "Withdrawn",
                ]),
                b1_title: "B.1 Course Learning Outcomes Assessment Results".into(),
                b1_columns: s([
                    "#",
                    "Course Learning Outcomes",
                    "Related PLOs Code",
                    "Assessment Methods",
                    "Targeted Level of Performance",
                    "Actual Level of Performance",
                    "Comment on Assessment Results",
                ]),
            },
            ar: Tp154Text {
                a1_title: "أ.1 توزيع التقديرات".into(),
                grade: "التقدير".into(),
                number_of_students: "عدد الطلاب".into(),
                percentage: "النسبة المئوية".into(),
                statuses: s(["محروم", "مستمر", "غير مكتمل", "ناجح", "راسب", "منسحب"]),
                b1_title: "ب.1 نتائج تقييم نواتج تعلم المقرر".into(),
                b1_columns: s([
                    "م",
                    "نواتج تعلم المقرر",
                    "رمز نواتج تعلم البرنامج المرتبطة",
                    "طرق التقييم",
                    "مستوى الأداء المستهدف",
                    "مستوى الأداء الفعلي",
                    "التعليق على نتائج التقييم",
                ]),
            },
            source: "built-in (spec §11.7 field names; replace with the pinned template package)"
                .into(),
        }
    }
}

/// TP-154 A.1 and B.1 for one course (or section, campus or the coordinator's combined view).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tp154<'a> {
    /// Header and provenance.
    pub meta: &'a ReportMeta,
    /// Wording.
    pub labels: &'a Tp154Labels,
    /// Header rows.
    pub headers: Headers,
}

impl Tp154<'_> {
    /// (is Arabic, wording) per header row.
    fn langs(&self) -> Vec<(bool, &Tp154Text)> {
        match self.headers {
            Headers::English => vec![(false, &self.labels.en)],
            Headers::Arabic => vec![(true, &self.labels.ar)],
            Headers::Both => vec![(false, &self.labels.en), (true, &self.labels.ar)],
        }
    }

    fn joined(&self, f: impl Fn(&Tp154Text) -> &str) -> String {
        self.langs()
            .into_iter()
            .map(|(_, t)| f(t))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// A.1 as a sheet: grade columns (A+ … F, then the six status columns); rows of counts and
    /// percentages of all listed students.
    pub fn a1_sheet(&self, a1: &A1Table) -> Sheet {
        let headers = self
            .langs()
            .into_iter()
            .map(|(arabic, t)| {
                let mut h = vec![t.grade.clone()];
                h.extend(a1.letters.iter().map(|c| {
                    if arabic {
                        c.arabic.clone()
                    } else {
                        c.label.clone()
                    }
                }));
                h.extend(t.statuses.iter().cloned());
                h
            })
            .collect();
        let cells = || a1.letters.iter().chain(a1.statuses.iter());
        let counts = std::iter::once(Cell::text(self.joined(|t| &t.number_of_students)))
            .chain(cells().map(|c| Cell::Int(c.count as i64)))
            .collect();
        let pcts = std::iter::once(Cell::text(self.joined(|t| &t.percentage)))
            .chain(cells().map(|c| Cell::percent(Some(exact_str(&c.percent, 1)), 1)))
            .collect();
        let mut widths = vec![22.0];
        widths.extend(std::iter::repeat_n(8.0, a1.letters.len()));
        widths.extend(std::iter::repeat_n(11.0, a1.statuses.len()));
        Sheet {
            name: "TP-154 A.1".into(),
            title: format!("{} — {}", self.meta.title, self.joined(|t| &t.a1_title)),
            headers,
            rows: vec![Row::new(counts), Row::new(pcts)],
            notes: vec![
                format!("Students listed: {} (percentage denominator).", a1.total),
                "Letters on the Saudi scale (CUA 1444/9/1); DN is reported as Denied Entry and \
                 counts as F only for GPA (§10.6)."
                    .into(),
                format!("Wording: {}.", self.labels.source),
            ],
            rtl: self.headers == Headers::Arabic,
            widths,
        }
    }

    /// B.1 as a sheet: one group row per NQF domain, then its CLO rows.
    pub fn b1_sheet(&self, b1: &B1Table) -> Sheet {
        let headers = self
            .langs()
            .into_iter()
            .map(|(_, t)| t.b1_columns.to_vec())
            .collect();
        let mut rows = Vec::new();
        for g in &b1.groups {
            let heading = match self.headers {
                Headers::English => g.heading_en.clone(),
                Headers::Arabic => g.heading_ar.clone(),
                Headers::Both => format!("{} / {}", g.heading_en, g.heading_ar),
            };
            rows.push(Row::group(vec![
                Cell::text(g.number.clone()),
                Cell::text(heading),
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
            ]));
            for r in &g.rows {
                rows.push(Row::new(vec![
                    Cell::text(r.number.clone()),
                    Cell::text(format!("{}: {}", r.clo, r.clo_text)),
                    Cell::text(r.plo_codes.join(", ")),
                    Cell::text(r.methods.clone()),
                    Cell::text(r.targeted.clone()),
                    Cell::text(r.actual.clone()),
                    Cell::text(r.comment.clone()),
                ]));
            }
        }
        let mut notes = vec![b1.footnote.clone()];
        if b1.draft_watermark {
            notes.insert(
                0,
                "DRAFT MAPPINGS — not for submission (SCO-032 open) / مسودة".into(),
            );
        }
        notes.push(format!("Wording: {}.", self.labels.source));
        Sheet {
            name: "TP-154 B.1".into(),
            title: format!("{} — {}", self.meta.title, self.joined(|t| &t.b1_title)),
            headers,
            rows,
            notes,
            rtl: self.headers == Headers::Arabic,
            widths: vec![6.0, 50.0, 14.0, 34.0, 14.0, 40.0, 40.0],
        }
    }

    /// A.1 as CSV.
    pub fn write_a1_csv<W: Write>(&self, w: &mut W, a1: &A1Table) -> Result<(), ReportError> {
        write_csv(w, &self.a1_sheet(a1))
    }

    /// B.1 as CSV.
    pub fn write_b1_csv<W: Write>(&self, w: &mut W, b1: &B1Table) -> Result<(), ReportError> {
        write_csv(w, &self.b1_sheet(b1))
    }

    /// A.1 and B.1 as one XLSX workbook.
    pub fn write_xlsx<W: Write>(
        &self,
        w: &mut W,
        a1: &A1Table,
        b1: &B1Table,
    ) -> Result<(), ReportError> {
        write_xlsx(w, self.meta, &[self.a1_sheet(a1), self.b1_sheet(b1)])
    }
}
