//! Remark Office OMR's graded exports (spec §3.5 item 3, PAR-09): keys, points, learning
//! objectives and student rows from the Excel workbooks Remark's Advanced Grade writes, for
//! the migration of a programme (`omr import remark`) and for the parallel run of §23.7
//! ([`crate::parallel`], `omr compare remark`).
//!
//! **Only the documented export is read** (§3.5 legal constraints): the workbook's
//! *Student Response Report* sheet (a header row `Student ID, Student Name, [Section No,
//! Gender,] Version1, Total, Percent, Grade, Answers1 … AnswersN`, then the `Answer Key` row,
//! then one row per student, then a `Mean` footer), its *Student Statistics Report* (the
//! learning-objective groups `Q1 (1-8)` with each student's `points / possible`) and its
//! *Test Statistics Report* (the class statistics Remark reported). Remark's binary formats
//! (`.rmx`, `.aky`) are never opened. Student names are never read.
//!
//! **Points.** Remark exports each student's total, not the points per question. They come
//! from the learning-objective groups when the workbook has them (a group's possible points
//! split equally over its questions), otherwise they are solved exactly from the students'
//! totals and which questions each earned ([`omr_scoring::solve_points`], exact rationals);
//! points the instructor gives are kept. Whatever the source, the points must reproduce
//! **every** row's Remark total exactly (and every group's points where the workbook has
//! them), or the import is refused with what to do (R10): never a fitted or rounded value.
//!
//! **Key sheets.** A row whose student ID is one repeated digit (`0000000000`), or whose
//! answers are the key with full marks, is the instructor's key sheet graded as a student: it
//! is reported, never counted as a student.

use crate::AppError;
use crate::exam::{
    self, CloDefinition, CloMapping, DomainLetter, FormKey, KeyFilePoints, OutcomeSetup, ParsedKey,
    PolicyChoice,
};
use omr_scoring::{Equation, Exact, PointsError, exact_from_f64, exact_text, solve_points};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The sheet with the answers (Remark's *Student Response Report*).
pub const STUDENT_RESPONSE: &str = "Student Response Report";
/// The sheet with the learning-objective groups (*Student Statistics Report*).
pub const STUDENT_STATISTICS: &str = "Student Statistics Report";
/// The sheet with the class statistics Remark reported (*Test Statistics Report*).
pub const TEST_STATISTICS: &str = "Test Statistics Report";
/// Largest workbook read (a graded export of a large class is well under 5 MB).
pub const MAX_WORKBOOK_BYTES: u64 = 64 << 20;
/// The status of an imported item → CLO row (§11.2).
pub const IMPORTED_STATUS: &str = "imported, needs review";
/// The source of an imported row (§11.2 `source`).
pub const IMPORTED_SOURCE: &str = "Remark import";

/// What to do with a workbook that is not Remark's graded export (R10).
const REMEDY_EXPORT: &str = "in Remark Office OMR, open the graded results and export the \
    Advanced Grade reports to Excel (.xlsx) with the Student Response Report (answers and \
    answer key) included; give that workbook";

/// One answer cell as Remark exported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemarkAnswer {
    /// One option (0-based).
    Option(u8),
    /// Empty cell: no answer.
    Blank,
    /// Several options.
    Multi(Vec<u8>),
    /// Anything else, as written (e.g. `?`).
    Other(String),
}

impl RemarkAnswer {
    /// Parses a cell: `B`, `b`, `AB`, `A,B`, `A|B`, empty.
    pub fn parse(s: &str) -> Self {
        let t = exam::clean_cell(s);
        let t = t.trim();
        // Remark writes an unmarked region as `BLANK` (or leaves the cell empty).
        if t.is_empty() || t.eq_ignore_ascii_case("blank") {
            return RemarkAnswer::Blank;
        }
        if let Some(o) = exam::parse_option(t, 10) {
            return RemarkAnswer::Option(o);
        }
        let parts: Vec<&str> = if t.contains([',', '|', '/', ';', ' ', '+']) {
            t.split([',', '|', '/', ';', ' ', '+'])
                .filter(|p| !p.is_empty())
                .collect()
        } else if t.chars().all(|c| c.is_ascii_alphabetic()) {
            t.char_indices()
                .map(|(i, c)| &t[i..i + c.len_utf8()])
                .collect()
        } else {
            Vec::new()
        };
        let opts: Option<BTreeSet<u8>> = parts.iter().map(|p| exam::parse_option(p, 10)).collect();
        match opts {
            Some(v) if v.len() > 1 => RemarkAnswer::Multi(v.into_iter().collect()),
            Some(v) if v.len() == 1 => RemarkAnswer::Option(*v.iter().next().expect("one")),
            _ => RemarkAnswer::Other(t.to_string()),
        }
    }

    /// Display text in the exports' convention: `B`, `BLANK`, `MULTI:AC`, or the cell.
    pub fn text(&self) -> String {
        let l = |o: &u8| char::from(b'A' + (*o).min(25)).to_string();
        match self {
            RemarkAnswer::Option(o) => l(o),
            RemarkAnswer::Blank => "BLANK".into(),
            RemarkAnswer::Multi(v) => format!("MULTI:{}", v.iter().map(l).collect::<String>()),
            RemarkAnswer::Other(s) => s.clone(),
        }
    }

    /// The same reading as a recognised response (option indices; blank; multi).
    pub fn same_as(&self, r: &omr_model::Response) -> bool {
        use omr_model::Response as R;
        match (self, r) {
            (RemarkAnswer::Option(a), R::Single(b)) => a == b,
            (RemarkAnswer::Blank, R::Blank) => true,
            (RemarkAnswer::Multi(a), R::Multi(b) | R::Set(b)) => {
                let mut b = b.clone();
                b.sort_unstable();
                b.dedup();
                *a == b
            }
            _ => false,
        }
    }
}

/// A learning-objective group of the Student Statistics Report: `Q1 (1-8)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoGroup {
    /// The objective's name (`Q1`).
    pub name: String,
    /// The header as Remark wrote it (`Q1 (1-8)`).
    pub label: String,
    /// Questions (1-based) in the group.
    pub questions: Vec<u16>,
    /// The group's possible points.
    pub possible: Exact,
}

/// Why a row is taken for the instructor's key sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeySheetKind {
    /// The student ID is one repeated digit (`0000000000`, `9999999999`): no student's.
    RepeatedDigitId,
    /// The answers are the key's with full marks under a student-like ID: a key sheet, or a
    /// student who answered every question right (the parallel run tells them apart by the
    /// scans).
    AnswersEqualKey,
}

/// One row of the Student Response Report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemarkRow {
    /// Worksheet row (1-based, as Excel shows it).
    pub row: u32,
    /// Student ID as text (`""` when the cell is empty).
    pub student_id: String,
    /// The ID came from a number cell (leading zeros may be lost).
    pub numeric_id: bool,
    /// `Section No`, when the export has it.
    pub section: String,
    /// `Version1` as written.
    pub version: String,
    /// Remark's total.
    pub total: Exact,
    /// Remark's letter grade (`Grade`), as written.
    #[serde(default)]
    pub grade: String,
    /// Answers 1…N.
    pub answers: Vec<RemarkAnswer>,
    /// The row's points per learning-objective group (workbook group order), when read.
    pub group_points: Vec<Option<Exact>>,
    /// Taken for a key sheet, and why.
    pub key_sheet: Option<KeySheetKind>,
}

/// The class statistics Remark reported (Test Statistics Report, *Overall* column).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemarkStats {
    /// Mean Score.
    pub mean: Option<f64>,
    /// Standard Deviation (Remark divides by N).
    pub sd: Option<f64>,
    /// Maximum Score.
    pub max: Option<f64>,
    /// Kuder-Richardson Formula 20.
    pub kr20: Option<f64>,
    /// Coefficient (Cronbach) Alpha.
    pub alpha: Option<f64>,
}

/// One graded workbook.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemarkWorkbook {
    /// File name (display only).
    pub file: String,
    /// SHA-384 of the file.
    pub sha384: String,
    /// Answer columns (questions).
    pub questions: usize,
    /// The Answer Key row's entries as key entries (`B`, `A|C`, `` missing).
    pub key: Vec<String>,
    /// The Answer Key row's total possible points.
    pub total_possible: Exact,
    /// The header has `Section No`.
    pub has_section: bool,
    /// The header has `Gender`.
    pub has_gender: bool,
    /// The header has a version column.
    pub has_version: bool,
    /// Student rows (key sheets included, flagged).
    pub rows: Vec<RemarkRow>,
    /// Footer rows (Mean …) skipped.
    pub footer_rows: usize,
    /// Learning-objective groups (empty when the workbook has none).
    pub groups: Vec<LoGroup>,
    /// The statistics Remark reported.
    pub stats: Option<RemarkStats>,
}

impl RemarkWorkbook {
    /// The key entries parsed (5 options at most on IAU sheets; up to 10 accepted).
    pub fn parsed_key(&self) -> Vec<ParsedKey> {
        self.key
            .iter()
            .map(|k| exam::parse_key_entry(k, 10).unwrap_or(ParsedKey::Missing))
            .collect()
    }

    /// The distinct `Version1` values of its rows (a version left blank, `BLANK` or
    /// unreadable on a sheet is not a version).
    pub fn versions(&self) -> BTreeSet<String> {
        self.rows
            .iter()
            .filter_map(|r| version_text(&r.version))
            .collect()
    }
}

/// A row's `Version1` as a version, `None` when the sheet's version was left blank,
/// double-marked or unreadable (`BLANK`, `MULTI`, `?`, empty).
pub fn version_text(v: &str) -> Option<String> {
    let t = v.trim().to_uppercase();
    (!t.is_empty() && !["BLANK", "MULTI", "MULTIPLE", "?", "*", "-"].contains(&t.as_str()))
        .then_some(t)
}

/// Whether answer `a` earns question `q` under `key`.
pub fn earns(key: &[ParsedKey], q: usize, a: &RemarkAnswer) -> bool {
    match (key.get(q), a) {
        (Some(ParsedKey::Options(v)), RemarkAnswer::Option(o)) => v.contains(o),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Reading a workbook.

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Empty,
    Num(f64),
    Text(String),
}

impl Val {
    fn text(&self) -> String {
        match self {
            Val::Empty => String::new(),
            Val::Num(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
            Val::Num(f) => format!("{f}"),
            Val::Text(s) => s.clone(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Val::Empty => true,
            Val::Text(s) => s.trim().is_empty(),
            Val::Num(_) => false,
        }
    }

    fn exact(&self) -> Option<Exact> {
        match self {
            Val::Num(f) => exact_from_f64(*f),
            Val::Text(s) => {
                let t = s.trim().replace(',', ".");
                omr_scoring::parse_points(&t)
                    .ok()
                    .map(|p| omr_scoring::widen(&p))
            }
            Val::Empty => None,
        }
    }

    fn f64(&self) -> Option<f64> {
        match self {
            Val::Num(f) => Some(*f),
            Val::Text(s) => s.trim().parse().ok(),
            Val::Empty => None,
        }
    }
}

/// A sheet as rows of values, with the worksheet row number (1-based) of each.
struct Grid {
    rows: Vec<(u32, Vec<Val>)>,
}

fn label_eq(v: &Val, want: &str) -> bool {
    matches!(v, Val::Text(s) if s.trim().eq_ignore_ascii_case(want))
}

fn sheet(
    wb: &mut calamine::Xlsx<std::io::Cursor<Vec<u8>>>,
    names: &[String],
    want: &str,
) -> Result<Option<Grid>, String> {
    use calamine::Data;
    let Some(name) = names.iter().find(|n| n.trim().eq_ignore_ascii_case(want)) else {
        return Ok(None);
    };
    let rows = crate::xlsx::sheet_rows(wb, name)?
        .into_iter()
        .map(|(rn, r)| {
            let cells = r
                .into_iter()
                .map(|c| match c {
                    Data::Int(i) => Val::Num(i as f64),
                    Data::Float(f) => Val::Num(f),
                    Data::String(s) => Val::Text(s),
                    Data::Bool(b) => Val::Text(b.to_string()),
                    Data::Empty => Val::Empty,
                    other => Val::Text(other.to_string()),
                })
                .collect();
            (rn, cells)
        })
        .collect();
    Ok(Some(Grid { rows }))
}

/// The ID cell as text: a number cell written without its `.0`, a `2210345601.0` text cell
/// repaired ([`exam::clean_roster_id`]), Arabic-Indic digits folded.
fn id_text(v: &Val) -> (String, bool) {
    match v {
        Val::Num(_) => (v.text(), true),
        Val::Text(s) => match exam::clean_roster_id(s, false) {
            Ok((id, numeric)) => (omr_identity::normalize_id(&id), numeric),
            Err(_) => (omr_identity::normalize_id(s), false),
        },
        Val::Empty => (String::new(), false),
    }
}

/// `Q1 (1-8)`, `LO 2 (11-18, 20)`, `(5)` → name and 1-based questions. `None` when the text
/// is not a group header (no question list in parentheses); an error when it is one but names
/// a question this export does not have (a Student Statistics Report of another test).
fn parse_group(label: &str, questions: usize) -> Option<Result<(String, Vec<u16>), String>> {
    let t = label.trim();
    let open = t.rfind('(')?;
    let inner = t[open + 1..].strip_suffix(')')?;
    if !inner.chars().any(|c| c.is_ascii_digit())
        || !inner
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '–' | ',' | ';' | ' '))
    {
        return None;
    }
    let name = t[..open].trim().to_string();
    let mut qs = BTreeSet::new();
    for part in inner
        .split([',', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let (a, b) = match part.split_once(['-', '–']) {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let (Ok(a), Ok(b)) = (a.parse::<u16>(), b.parse::<u16>()) else {
            return Some(Err(format!("`{t}`: `{part}` is not a question range")));
        };
        if a == 0 || b < a || usize::from(b) > questions {
            return Some(Err(format!(
                "`{t}` names questions {a}-{b} and the Student Response Report has {questions}"
            )));
        }
        qs.extend(a..=b);
    }
    (!qs.is_empty()).then(|| Ok((name, qs.into_iter().collect())))
}

/// Labels of the statistics rows Remark writes below the students.
const FOOTER_LABELS: &[&str] = &[
    "mean",
    "median",
    "mode",
    "average",
    "standard deviation",
    "std dev",
    "std. dev.",
    "variance",
    "minimum",
    "maximum",
    "min",
    "max",
    "count",
    "range",
    "total",
    "percent",
];

/// A text ID cell with no digit (`Mean`, `ERROR`, `BLANK`).
fn wordy(v: &Val) -> bool {
    matches!(v, Val::Text(s)
        if !s.trim().is_empty() && !s.chars().any(|c| c.is_ascii_digit() || ('٠'..='٩').contains(&c)))
}

/// A row below the students (Mean, Median …): a word for its ID, no answer letters, and
/// statistics in the answer columns, a known statistic's label or no total. A student row
/// whose ID Remark could not read (`ERROR`, `BLANK`) has answers or a total, and is read.
fn footer_row(id: &Val, answers: &[Val], total: &Val) -> bool {
    if !wordy(id) {
        return false;
    }
    let mut numbers = false;
    for v in answers {
        match v {
            Val::Num(_) => numbers = true,
            Val::Empty => {}
            Val::Text(x) if x.trim().is_empty() => {}
            Val::Text(x) if x.trim().replace(',', ".").parse::<f64>().is_ok() => numbers = true,
            Val::Text(_) => return false,
        }
    }
    let label = id.text().trim().to_lowercase();
    numbers || FOOTER_LABELS.contains(&label.as_str()) || total.exact().is_none()
}

/// A student ID Excel turned into a number in scientific notation that lost digits
/// (`2.21035E+09`): no student number can be recovered from it (spec §3.5: Excel-mangled IDs).
pub fn id_lost_digits(id: &str) -> bool {
    exam::clean_roster_id(id, false).is_err()
        && id.contains(['e', 'E'])
        && id.chars().any(|c| c.is_ascii_digit())
}

const FOOTER_HINT: &str = "rows below the students (Mean, Median …) are skipped";

/// Reads one graded workbook (`name` is for messages only). Refuses, with what to do, a file
/// that is not an `.xlsx` workbook or has no Student Response Report with an Answer Key row.
pub fn read_workbook(name: &str, bytes: &[u8]) -> Result<RemarkWorkbook, AppError> {
    let bad = |m: String| AppError::Input(format!("{name}: {m}"));
    if bytes.len() as u64 > MAX_WORKBOOK_BYTES {
        return Err(bad(format!(
            "{} bytes: larger than any graded export ({} MB at most); {REMEDY_EXPORT}",
            bytes.len(),
            MAX_WORKBOOK_BYTES >> 20
        )));
    }
    if !bytes.starts_with(b"PK") {
        return Err(bad(format!(
            "not an Excel .xlsx workbook (Remark's binary .rmx and .aky files are never read); \
             {REMEDY_EXPORT}"
        )));
    }
    use calamine::Reader;
    let mut wb = crate::xlsx::open(bytes).map_err(|e| bad(format!("{e}; {REMEDY_EXPORT}")))?;
    let names = wb.sheet_names();
    let resp = sheet(&mut wb, &names, STUDENT_RESPONSE)
        .map_err(|e| bad(format!("{STUDENT_RESPONSE}: {e}")))?
        .ok_or_else(|| {
            bad(format!(
                "no `{STUDENT_RESPONSE}` sheet (sheets: {}); {REMEDY_EXPORT}",
                names.join(", ")
            ))
        })?;
    // Header row: `Student ID` … `Total` … `Answers1`.
    let hi = resp
        .rows
        .iter()
        .take(40)
        .position(|(_, r)| r.iter().any(|c| label_eq(c, "Student ID")))
        .ok_or_else(|| {
            bad(format!(
                "{STUDENT_RESPONSE}: no header row with `Student ID`; {REMEDY_EXPORT}"
            ))
        })?;
    let header: Vec<String> = resp.rows[hi]
        .1
        .iter()
        .map(|c| c.text().trim().to_string())
        .collect();
    let find = |names: &[&str]| {
        header
            .iter()
            .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
    };
    let c_id = find(&["Student ID"]).expect("found above");
    let c_total = find(&["Total", "Total Score", "Score"]).ok_or_else(|| {
        bad(format!(
            "{STUDENT_RESPONSE}: no `Total` column; {REMEDY_EXPORT}"
        ))
    })?;
    let c_section = find(&["Section No", "Section", "Section Number"]);
    let c_gender = find(&["Gender"]);
    let c_grade = find(&["Grade"]);
    let c_version = find(&["Version1", "Version", "Form", "Version 1"]);
    let mut answer_cols: Vec<(usize, usize)> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let l = h.to_ascii_lowercase();
            let d = l
                .strip_prefix("answers")
                .or_else(|| l.strip_prefix("answer"))?
                .trim();
            d.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| (n, i))
        })
        .collect();
    answer_cols.sort_unstable();
    if answer_cols.is_empty() {
        return Err(bad(format!(
            "{STUDENT_RESPONSE}: no `Answers1` … columns; {REMEDY_EXPORT}"
        )));
    }
    if answer_cols
        .iter()
        .enumerate()
        .any(|(k, (n, _))| *n != k + 1)
    {
        return Err(bad(format!(
            "{STUDENT_RESPONSE}: the answer columns are not Answers1…Answers{} in order; \
             {REMEDY_EXPORT}",
            answer_cols.len()
        )));
    }
    let n = answer_cols.len();
    let cell = |r: &[Val], c: usize| r.get(c).cloned().unwrap_or(Val::Empty);
    // The Answer Key row: the first non-empty row below the header.
    let mut k = hi + 1;
    while k < resp.rows.len() && resp.rows[k].1.iter().all(Val::is_empty) {
        k += 1;
    }
    let (_, key_row) = resp
        .rows
        .get(k)
        .filter(|(_, r)| label_eq(&cell(r, c_id), "Answer Key"))
        .ok_or_else(|| {
            bad(format!(
                "{STUDENT_RESPONSE}: no `Answer Key` row below the header; export the report \
                 with the answer key; {REMEDY_EXPORT}"
            ))
        })?;
    let total_possible = cell(key_row, c_total).exact().ok_or_else(|| {
        bad(format!(
            "{STUDENT_RESPONSE}: the Answer Key row has no total possible points in `Total`; \
             {REMEDY_EXPORT}"
        ))
    })?;
    let mut key = Vec::with_capacity(n);
    for (q, (_, c)) in answer_cols.iter().enumerate() {
        let t = cell(key_row, *c).text().trim().to_string();
        // Remark's AND key (`A+C`, `A&C`, `A and C`) is never read as "either answer".
        if t.contains(['+', '&']) || format!(" {} ", t.to_lowercase()).contains(" and ") {
            exam::parse_key_entry(&t, 10).map_err(|m| {
                bad(format!(
                    "{STUDENT_RESPONSE}: answer key of question {}: {m}; correct the key in \
                     Remark or give the key as a file",
                    q + 1
                ))
            })?;
        }
        let entry = match RemarkAnswer::parse(&t) {
            RemarkAnswer::Blank => String::new(),
            RemarkAnswer::Option(o) => char::from(b'A' + o).to_string(),
            RemarkAnswer::Multi(v) => v
                .iter()
                .map(|o| char::from(b'A' + o).to_string())
                .collect::<Vec<_>>()
                .join("|"),
            RemarkAnswer::Other(s) => {
                exam::parse_key_entry(&s, 10).map_err(|m| {
                    bad(format!(
                        "{STUDENT_RESPONSE}: answer key of question {}: {m}",
                        q + 1
                    ))
                })?;
                s
            }
        };
        key.push(entry);
    }
    let parsed: Vec<ParsedKey> = key
        .iter()
        .map(|k| exam::parse_key_entry(k, 10).unwrap_or(ParsedKey::Missing))
        .collect();
    // Student rows; the statistics rows below them (Mean …) are skipped, each row judged on
    // its own.
    let mut rows = Vec::new();
    let mut footer_rows = 0usize;
    for (rn, r) in &resp.rows[k + 1..] {
        if r.iter().all(Val::is_empty) {
            continue;
        }
        let idc = cell(r, c_id);
        if label_eq(&idc, "Answer Key") {
            return Err(bad(format!(
                "{STUDENT_RESPONSE} row {rn}: a second `Answer Key` row (the report holds \
                 several tests or versions); export one workbook per version"
            )));
        }
        let answer_cells: Vec<Val> = answer_cols.iter().map(|(_, c)| cell(r, *c)).collect();
        if footer_row(&idc, &answer_cells, &cell(r, c_total)) {
            footer_rows += 1;
            continue;
        }
        let total = cell(r, c_total).exact().ok_or_else(|| {
            bad(format!(
                "{STUDENT_RESPONSE} row {rn}: `Total` is not a number; {FOOTER_HINT}, every \
                 student row needs its total"
            ))
        })?;
        let (student_id, numeric_id) = id_text(&idc);
        let answers: Vec<RemarkAnswer> = answer_cells
            .iter()
            .map(|v| RemarkAnswer::parse(&v.text()))
            .collect();
        let repeated = !student_id.is_empty()
            && student_id
                .chars()
                .next()
                .is_some_and(|c0| c0.is_ascii_digit() && student_id.chars().all(|c| c == c0));
        let all_key = (0..n).all(|q| earns(&parsed, q, &answers[q]));
        let key_sheet = if repeated {
            Some(KeySheetKind::RepeatedDigitId)
        } else if all_key && total == total_possible {
            Some(KeySheetKind::AnswersEqualKey)
        } else {
            None
        };
        rows.push(RemarkRow {
            row: *rn,
            student_id,
            numeric_id,
            section: c_section
                .map(|c| cell(r, c).text().trim().to_string())
                .unwrap_or_default(),
            version: c_version
                .map(|c| cell(r, c).text().trim().to_string())
                .unwrap_or_default(),
            total,
            grade: c_grade
                .map(|c| cell(r, c).text().trim().to_string())
                .unwrap_or_default(),
            answers,
            group_points: Vec::new(),
            key_sheet,
        });
    }
    let mut out = RemarkWorkbook {
        file: name.to_string(),
        sha384: omr_store::jcs::sha384_hex(bytes),
        questions: n,
        key,
        total_possible,
        has_section: c_section.is_some(),
        has_gender: c_gender.is_some(),
        has_version: c_version.is_some(),
        rows,
        footer_rows,
        groups: Vec::new(),
        stats: None,
    };
    if let Some(g) = sheet(&mut wb, &names, STUDENT_STATISTICS)
        .map_err(|e| bad(format!("{STUDENT_STATISTICS}: {e}")))?
    {
        read_groups(name, &g, &mut out)?;
    }
    if let Some(g) = sheet(&mut wb, &names, TEST_STATISTICS).ok().flatten() {
        out.stats = read_stats(&g);
    }
    Ok(out)
}

/// The learning-objective groups and each row's points per group (Student Statistics Report:
/// a row with `Overall` and the group headers, a `Student ID` header row, then per student and
/// group `<points> / <possible>`).
fn read_groups(name: &str, g: &Grid, wb: &mut RemarkWorkbook) -> Result<(), AppError> {
    let bad = |m: String| AppError::Input(format!("{name}: {STUDENT_STATISTICS}: {m}"));
    let Some(gi) = g
        .rows
        .iter()
        .position(|(_, r)| r.iter().any(|c| label_eq(c, "Overall")))
    else {
        return Ok(());
    };
    let mut groups: Vec<(usize, String, String, Vec<u16>)> = Vec::new();
    for (j, c) in g.rows[gi].1.iter().enumerate() {
        if let Val::Text(s) = c
            && let Some(parsed) = parse_group(s, wb.questions)
        {
            let (gname, qs) = parsed.map_err(|m| {
                bad(format!(
                    "learning objective {m}: the two reports are not of one test; \
                     {REMEDY_EXPORT}"
                ))
            })?;
            groups.push((j, s.trim().to_string(), gname, qs));
        }
    }
    if groups.is_empty() {
        return Ok(());
    }
    let Some(hi) = g.rows[gi..]
        .iter()
        .position(|(_, r)| r.iter().any(|c| label_eq(c, "Student ID")))
        .map(|p| p + gi)
    else {
        return Err(bad(format!(
            "learning-objective groups but no `Student ID` header row; {REMEDY_EXPORT}"
        )));
    };
    let c_id = g.rows[hi]
        .1
        .iter()
        .position(|c| label_eq(c, "Student ID"))
        .expect("found above");
    // Per student row (in order, matched to the response rows by ID and occurrence): the
    // group's `points / possible` after its header column.
    /// A group's `points / possible` on one row.
    type Cells = Vec<(Option<Exact>, Option<Exact>)>;
    let mut per_id: BTreeMap<String, Vec<Cells>> = BTreeMap::new();
    for (_, r) in &g.rows[hi + 1..] {
        let Some(idc) = r.get(c_id) else {
            continue;
        };
        if matches!(idc, Val::Text(s) if !s.chars().any(|c| c.is_ascii_digit())) || idc.is_empty() {
            continue;
        }
        let (id, _) = id_text(idc);
        let mut v = Vec::new();
        for (k, (j, ..)) in groups.iter().enumerate() {
            let end = groups.get(k + 1).map_or(r.len(), |x| x.0);
            let slash = (*j..end.min(r.len())).find(|&i| label_eq(&r[i], "/"));
            v.push(match slash {
                Some(i) if i >= 1 => (
                    r.get(i - 1).and_then(Val::exact),
                    r.get(i + 1).and_then(Val::exact),
                ),
                _ => (None, None),
            });
        }
        per_id.entry(id).or_default().push(v);
    }
    // Possible points per group: one value for every student.
    let mut possible: Vec<Option<Exact>> = vec![None; groups.len()];
    for rows in per_id.values() {
        for v in rows {
            for (k, (_, p)) in v.iter().enumerate() {
                if let Some(p) = p {
                    match possible[k] {
                        None => possible[k] = Some(*p),
                        Some(x) if x != *p => {
                            return Err(bad(format!(
                                "group `{}` has {} possible points for one student and {} for \
                                 another; {REMEDY_EXPORT}",
                                groups[k].1,
                                exact_text(&x),
                                exact_text(p)
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    for row in &mut wb.rows {
        let k = used.entry(row.student_id.clone()).or_insert(0);
        row.group_points = per_id
            .get(&row.student_id)
            .and_then(|v| v.get(*k))
            .map(|v| v.iter().map(|(p, _)| *p).collect())
            .unwrap_or_else(|| vec![None; groups.len()]);
        *k += 1;
    }
    wb.groups = groups
        .into_iter()
        .zip(possible)
        .filter_map(|((_, label, gname, questions), p)| {
            p.map(|possible| LoGroup {
                name: gname,
                label,
                questions,
                possible,
            })
        })
        .collect();
    Ok(())
}

/// Remark's reported class statistics (*Overall* column of the Test Statistics Report).
fn read_stats(g: &Grid) -> Option<RemarkStats> {
    let (hi, col) = g.rows.iter().enumerate().find_map(|(i, (_, r))| {
        r.iter()
            .position(|c| label_eq(c, "Overall"))
            .map(|c| (i, c))
    })?;
    let mut s = RemarkStats::default();
    for (_, r) in &g.rows[hi + 1..] {
        let Some(Val::Text(l)) = r.first() else {
            continue;
        };
        let v = r.get(col).and_then(Val::f64);
        match l.trim() {
            "Mean Score" => s.mean = v,
            "Standard Deviation" => s.sd = v,
            "Maximum Score" => s.max = v,
            "Kuder-Richardson Formula 20" => s.kr20 = v,
            "Coefficient (Cronbach) Alpha" => s.alpha = v,
            _ => {}
        }
    }
    Some(s)
}

/// Reads workbook files (each checked for size before it is read).
pub fn read_workbook_files(paths: &[PathBuf]) -> Result<Vec<RemarkWorkbook>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Input(format!(
            "no Remark workbook given; {REMEDY_EXPORT}"
        )));
    }
    let mut out = Vec::new();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string());
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "rmx" | "aky" | "rmt" | "xls" | "xlsm") {
            return Err(AppError::Input(format!(
                "{name}: .{ext} files are not read (Remark's own formats are never opened; .xls \
                 and .xlsm workbooks are refused); {REMEDY_EXPORT}"
            )));
        }
        let meta = std::fs::metadata(p).map_err(|e| {
            AppError::Input(format!("{}: {e}; check the path of the file", p.display()))
        })?;
        if meta.len() > MAX_WORKBOOK_BYTES {
            return Err(AppError::Input(format!(
                "{name}: {} bytes, larger than any graded export; {REMEDY_EXPORT}",
                meta.len()
            )));
        }
        let bytes = std::fs::read(p).map_err(|e| {
            AppError::Input(format!("{}: {e}; check the path of the file", p.display()))
        })?;
        out.push(read_workbook(&name, &bytes)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Import: one key per version, exact points, learning objectives.

/// Where a version's points came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointsSource {
    /// Every question's points from its learning-objective group (equal split).
    LearningObjectives,
    /// Solved exactly from the totals (and the group points, when the equal split did not
    /// hold).
    SolvedFromTotals,
    /// Given by the instructor for every question.
    Given,
    /// Some given or from groups, the rest solved.
    Mixed,
}

impl PointsSource {
    /// Display text.
    pub fn describe(self) -> &'static str {
        match self {
            PointsSource::LearningObjectives => {
                "from the learning-objective groups (each group's points split equally over its \
                 questions, the only split the totals allow)"
            }
            PointsSource::SolvedFromTotals => "solved exactly from the students' totals",
            PointsSource::Given => "given by the instructor",
            PointsSource::Mixed => {
                "partly given or from the learning-objective groups, the rest solved exactly \
                 from the totals"
            }
        }
    }
}

/// One version of the imported exam.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedVersion {
    /// Form letter (`A`).
    pub form: String,
    /// `Version1` as Remark wrote it.
    pub remark_version: String,
    /// The workbooks (file names) that give this version.
    pub workbooks: Vec<String>,
    /// Key entries per question.
    pub key: Vec<String>,
    /// Exact points per question.
    pub points: Vec<Exact>,
    /// Where they came from.
    pub points_source: PointsSource,
    /// The Answer Key row's total possible points.
    pub total_possible: Exact,
    /// Learning-objective groups (empty without).
    pub groups: Vec<LoGroup>,
    /// Student rows (key sheets excluded).
    pub students: usize,
    /// Rows taken for key sheets.
    pub key_sheet_rows: usize,
    /// Rows whose Remark total the points reproduce (all of them, or the import is refused).
    pub totals_reproduced: usize,
    /// Rows checked (students and key sheets).
    pub rows_checked: usize,
    /// Group points checked (row × group).
    pub group_points_checked: usize,
}

/// A row taken for a key sheet (reported, never a student).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySheetRowInfo {
    /// Workbook.
    pub file: String,
    /// Worksheet row.
    pub row: u32,
    /// The ID masked to its last 3 digits.
    pub masked_id: String,
    /// Why.
    pub kind: KeySheetKind,
    /// Its answers are the version's key.
    pub answers_equal_key: bool,
    /// Its total is the total possible.
    pub full_marks: bool,
}

/// One workbook as imported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkbookInfo {
    /// File name.
    pub file: String,
    /// SHA-384.
    pub sha384: String,
    /// Form letter.
    pub form: String,
    /// Student rows (key sheets excluded).
    pub students: usize,
    /// Key-sheet rows.
    pub key_sheets: usize,
    /// Footer rows skipped.
    pub footer_rows: usize,
    /// Header variant: `Section No` present.
    pub has_section: bool,
    /// Header variant: `Gender` present.
    pub has_gender: bool,
    /// Learning-objective groups.
    pub groups: usize,
}

/// A Remark import: the key per version with exact points, the learning objectives as
/// proposed CLO mappings, and what was checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemarkImport {
    /// Questions per version.
    pub questions: u16,
    /// Versions in letter order.
    pub versions: Vec<ImportedVersion>,
    /// Workbooks.
    pub workbooks: Vec<WorkbookInfo>,
    /// Rows taken for key sheets.
    pub key_sheet_rows: Vec<KeySheetRowInfo>,
    /// Findings for the summary (no student values, no key letters).
    pub notes: Vec<String>,
}

/// Points the instructor gives (`read_points_file`): form letter (`""` every version) →
/// one optional exact value per question.
pub type GivenPoints = BTreeMap<String, Vec<Option<String>>>;

fn masked(id: &str) -> String {
    if id.is_empty() {
        "(blank)".into()
    } else {
        crate::export::mask_numbers(id)
    }
}

/// The form letter of Remark's version text: a letter, or a digit `1`… (`1` → `A`).
fn form_of(v: &str) -> Option<String> {
    let t = v.trim().to_uppercase();
    let t = t
        .strip_prefix("VERSION")
        .or_else(|| t.strip_prefix("FORM"))
        .unwrap_or(&t)
        .trim()
        .to_string();
    if t.len() == 1 {
        let c = t.as_bytes()[0];
        if c.is_ascii_uppercase() {
            return Some(t);
        }
        if (b'1'..=b'9').contains(&c) {
            return Some(char::from(b'A' + (c - b'1')).to_string());
        }
    }
    None
}

fn list(qs: &[usize]) -> String {
    let v: Vec<String> = qs.iter().map(|q| (q + 1).to_string()).collect();
    if v.len() > 12 {
        format!("{} … ({} questions)", v[..12].join(", "), v.len())
    } else {
        v.join(", ")
    }
}

/// One version after pass 1: its workbooks agree; its points derived (or why not).
struct Prepared {
    form: String,
    idx: Vec<usize>,
    key: Vec<ParsedKey>,
    eqs: Vec<Equation>,
    where_: Vec<(usize, u32)>,
    derived: Result<(Vec<Exact>, PointsSource), PointsError>,
}

/// Pass 1 of an import: the versions found and, per version, the points derived.
struct Prepped {
    prepared: Vec<Prepared>,
    version_of: Vec<Option<(String, String)>>,
    notes: Vec<String>,
    questions: u16,
}

/// Another version's points left for `pr`'s undetermined questions when the versions are
/// the same questions in another order.
fn completion(pr: &Prepared, prepared: &[Prepared]) -> Option<(String, Vec<Exact>)> {
    let Err(PointsError::Underdetermined {
        questions,
        determined,
    }) = &pr.derived
    else {
        return None;
    };
    prepared.iter().filter(|o| o.form != pr.form).find_map(|o| {
        let (pts, _) = o.derived.as_ref().ok()?;
        let mut left: Vec<Exact> = pts.clone();
        for d in determined.iter().flatten() {
            let i = left.iter().position(|x| x == d)?;
            left.remove(i);
        }
        (left.len() == questions.len()).then(|| (o.form.clone(), left))
    })
}

fn prepare(workbooks: &[RemarkWorkbook], given: &GivenPoints) -> Result<Prepped, AppError> {
    let first = workbooks
        .first()
        .ok_or_else(|| AppError::Input(format!("no Remark workbook given; {REMEDY_EXPORT}")))?;
    let n = first.questions;
    for (k, w) in workbooks.iter().enumerate() {
        if let Some(o) = workbooks[..k].iter().find(|o| o.sha384 == w.sha384) {
            return Err(AppError::Input(format!(
                "{} and {} are the same workbook (the same SHA-384): its rows would count \
                 twice; give each workbook once",
                o.file, w.file
            )));
        }
    }
    if let Some(w) = workbooks.iter().find(|w| w.questions != n) {
        return Err(AppError::Input(format!(
            "{} has {} questions and {} has {}: these workbooks are not one exam; import each \
             exam on its own",
            first.file, n, w.file, w.questions
        )));
    }
    let questions = u16::try_from(n)
        .ok()
        .filter(|q| *q <= 200)
        .ok_or_else(|| AppError::Input(format!("{}: {n} questions", first.file)))?;
    // The version of each workbook: its rows' Version1, else the version whose key it shares.
    let mut version_of: Vec<Option<(String, String)>> = Vec::new();
    for w in workbooks {
        let vs = w.versions();
        if vs.len() > 1 {
            return Err(AppError::Input(format!(
                "{} lists students of versions {} under one Answer Key row (Remark's version \
                 mapping): export one workbook per version (Advanced Grade, one test per \
                 version) and import them together",
                w.file,
                vs.iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        version_of.push(match vs.into_iter().next() {
            Some(v) => Some((
                form_of(&v).ok_or_else(|| {
                    AppError::Input(format!(
                        "{}: version `{v}` is not a letter A–Z or a number 1–9",
                        w.file
                    ))
                })?,
                v,
            )),
            None => None,
        });
    }
    let mut notes = Vec::new();
    if version_of.iter().all(Option::is_none) {
        // No version column filled: one version, or several told apart by their keys.
        let mut keys: Vec<&Vec<String>> = Vec::new();
        for (w, v) in workbooks.iter().zip(version_of.iter_mut()) {
            let k = keys.iter().position(|k| **k == w.key).unwrap_or_else(|| {
                keys.push(&w.key);
                keys.len() - 1
            });
            *v = Some((
                char::from(b'A' + (k as u8).min(25)).to_string(),
                String::new(),
            ));
        }
        if keys.len() > 1 {
            notes.push(format!(
                "no workbook says its version: the {} different answer keys were taken as \
                 versions {} in file order; check the version map",
                keys.len(),
                (0..keys.len())
                    .map(|k| char::from(b'A' + k as u8).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else {
        for k in 0..workbooks.len() {
            if version_of[k].is_some() {
                continue;
            }
            let w = &workbooks[k];
            let same: BTreeSet<(String, String)> = workbooks
                .iter()
                .zip(&version_of)
                .filter(|(o, v)| v.is_some() && o.key == w.key)
                .filter_map(|(_, v)| v.clone())
                .collect();
            if same.len() != 1 {
                return Err(AppError::Input(format!(
                    "{} does not say its version (the Version1 column is empty) and its answer \
                     key matches {} of the other workbooks' versions; export it again with the \
                     version column",
                    w.file,
                    same.len()
                )));
            }
            notes.push(format!(
                "{}: no version column filled; taken as version {} (same answer key)",
                w.file,
                same.iter().next().map(|v| v.0.clone()).unwrap_or_default()
            ));
            version_of[k] = same.into_iter().next();
        }
    }
    let forms: BTreeSet<String> = version_of
        .iter()
        .filter_map(|v| v.as_ref().map(|v| v.0.clone()))
        .collect();
    if let Some(f) = given.keys().find(|f| !f.is_empty() && !forms.contains(*f)) {
        return Err(AppError::Input(format!(
            "points are given for version {f}, which the workbooks do not have (versions {}); \
             rename the column (points_A …) or remove it",
            forms.iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    // Pass 1, per version: the workbooks agree; the points are derived.
    let mut prepared: Vec<Prepared> = Vec::new();
    for form in &forms {
        let idx: Vec<usize> = (0..workbooks.len())
            .filter(|k| version_of[*k].as_ref().is_some_and(|v| &v.0 == form))
            .collect();
        agree(form, &idx, workbooks)?;
        let lead = &workbooks[idx[0]];
        let key = lead.parsed_key();
        // Equations: every student row's total, and the total possible (Σ points).
        let mut eqs: Vec<Equation> = Vec::new();
        let mut group_eqs: Vec<Equation> = Vec::new();
        let mut where_: Vec<(usize, u32)> = Vec::new();
        for &k in &idx {
            for r in workbooks[k].rows.iter().filter(|r| r.key_sheet.is_none()) {
                let qs: Vec<usize> = (0..n).filter(|q| earns(&key, *q, &r.answers[*q])).collect();
                for (g, gp) in lead.groups.iter().zip(&r.group_points) {
                    if let Some(gp) = gp {
                        group_eqs.push(Equation {
                            questions: qs
                                .iter()
                                .copied()
                                .filter(|q| g.questions.contains(&(*q as u16 + 1)))
                                .collect(),
                            total: *gp,
                        });
                    }
                }
                eqs.push(Equation {
                    questions: qs,
                    total: r.total,
                });
                where_.push((k, r.row));
            }
        }
        eqs.push(Equation {
            questions: (0..n).collect(),
            total: lead.total_possible,
        });
        where_.push((idx[0], 0));
        // Given points: this version's column, else the column for every version.
        let mut given_v: Vec<Option<Exact>> = vec![None; n];
        for (q, slot) in given_v.iter_mut().enumerate() {
            let p = given
                .get(form.as_str())
                .and_then(|c| c.get(q).cloned().flatten())
                .or_else(|| given.get("").and_then(|c| c.get(q).cloned().flatten()));
            if let Some(p) = p {
                *slot = Some(omr_scoring::widen(
                    &omr_scoring::parse_points(&p)
                        .map_err(|e| AppError::Input(format!("given points `{p}`: {e}")))?,
                ));
            }
        }
        // From the groups: equal split (when every group covering the question agrees).
        let mut from_groups: Vec<Option<Exact>> = vec![None; n];
        for (q, slot) in from_groups.iter_mut().enumerate() {
            let vals: BTreeSet<Exact> = lead
                .groups
                .iter()
                .filter(|g| g.questions.contains(&(q as u16 + 1)))
                .map(|g| g.possible / Exact::from_integer(g.questions.len() as i128))
                .collect();
            if vals.len() == 1 {
                *slot = vals.into_iter().next();
            }
        }
        let known: Vec<Option<Exact>> = (0..n).map(|q| given_v[q].or(from_groups[q])).collect();
        let n_given = given_v.iter().flatten().count();
        let n_groups = (0..n)
            .filter(|q| given_v[*q].is_none() && from_groups[*q].is_some())
            .count();
        let derived = match solve_points(n, &eqs, &known) {
            Ok(p) => {
                let solved = n - n_given - n_groups;
                let src = if n_given == n {
                    PointsSource::Given
                } else if n_groups == n {
                    PointsSource::LearningObjectives
                } else if solved == n {
                    PointsSource::SolvedFromTotals
                } else {
                    PointsSource::Mixed
                };
                Ok((p, src))
            }
            Err(e) if n_groups > 0 => {
                // The equal split did not hold: solve with the group points as equations.
                let mut all = eqs.clone();
                all.extend(group_eqs.iter().cloned());
                for g in &lead.groups {
                    all.push(Equation {
                        questions: g.questions.iter().map(|q| usize::from(*q) - 1).collect(),
                        total: g.possible,
                    });
                }
                match solve_points(n, &all, &given_v) {
                    Ok(p) => {
                        notes.push(if matches!(e, PointsError::Inconsistent { .. }) {
                            format!(
                                "version {form}: the learning-objective points are not split \
                                 equally within their groups; the points were solved exactly \
                                 from the totals and the group points instead"
                            )
                        } else {
                            format!(
                                "version {form}: the points were solved exactly from the totals \
                                 with each student's learning-objective points as equations too"
                            )
                        });
                        Ok((p, PointsSource::SolvedFromTotals))
                    }
                    Err(_) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        // The equal split must be the only points the export allows: when the totals and the
        // group points cannot tell some of a group's questions apart (answered right by
        // exactly the same students), any split fits and the equal one is a guess, refused
        // as the totals' own gaps are.
        let derived = match derived {
            Ok(found) if n_groups > 0 => {
                let mut all = eqs.clone();
                all.extend(group_eqs.iter().cloned());
                for g in &lead.groups {
                    all.push(Equation {
                        questions: g.questions.iter().map(|q| usize::from(*q) - 1).collect(),
                        total: g.possible,
                    });
                }
                match solve_points(n, &all, &given_v) {
                    Err(PointsError::Underdetermined {
                        questions,
                        determined,
                    }) if questions.iter().any(|q| from_groups[*q].is_some()) => {
                        Err(PointsError::Underdetermined {
                            questions,
                            determined,
                        })
                    }
                    _ => Ok(found),
                }
            }
            other => other,
        };
        prepared.push(Prepared {
            form: form.clone(),
            idx,
            key,
            eqs,
            where_,
            derived,
        });
    }
    Ok(Prepped {
        prepared,
        version_of,
        notes,
        questions,
    })
}

/// The points of questions the totals leave undetermined in one version: the value another,
/// fully determined version leaves for each of them (the versions being the same questions in
/// another order), else the equal split of the learning objective that holds the question
/// (the totals fix only the objective's sum): `given` plus those values, per version. This is
/// the suggestion a refusal shows; the instructor gives the points (`--points`, or the
/// desktop's explicit choice), never the import on its own.
pub fn suggest_points(
    workbooks: &[RemarkWorkbook],
    given: &GivenPoints,
) -> Result<GivenPoints, AppError> {
    let pp = prepare(workbooks, given)?;
    let n = usize::from(pp.questions);
    let mut out = given.clone();
    for pr in &pp.prepared {
        let Err(PointsError::Underdetermined { questions, .. }) = &pr.derived else {
            continue;
        };
        let col = out.entry(pr.form.clone()).or_insert_with(|| vec![None; n]);
        // Another, fully determined version leaves one value for them.
        if let Some((_, left)) = completion(pr, &pp.prepared) {
            let vals: BTreeSet<&Exact> = left.iter().collect();
            if vals.len() == 1 {
                for &q in questions {
                    col[q] = Some(exact_text(&left[0]));
                }
                continue;
            }
        }
        // Else the equal split of the learning objective that holds each question (one
        // objective; the totals only fix the objective's sum).
        let lead = &workbooks[pr.idx[0]];
        for &q in questions {
            let mut split = lead
                .groups
                .iter()
                .filter(|g| g.questions.contains(&(q as u16 + 1)))
                .map(|g| g.possible / Exact::from_integer(g.questions.len() as i128));
            if let (Some(v), None) = (split.next(), split.next()) {
                col[q] = Some(exact_text(&v));
            }
        }
    }
    out.retain(|_, c| c.iter().any(Option::is_some));
    Ok(out)
}

/// Imports Remark workbooks of one exam: every workbook of a version must give the same key,
/// total possible and learning-objective groups (else refused naming the files); the points
/// of each version are derived (groups, given, solved) and checked against every row's total.
pub fn import(workbooks: &[RemarkWorkbook], given: &GivenPoints) -> Result<RemarkImport, AppError> {
    let Prepped {
        prepared,
        version_of,
        mut notes,
        questions,
    } = prepare(workbooks, given)?;
    let n = usize::from(questions);
    // A refusal names what to do; for questions the totals cannot tell apart, the points a
    // fully determined version leaves for them when the versions are one exam in two orders.
    for pr in &prepared {
        let Err(e) = &pr.derived else { continue };
        return Err(AppError::Input(describe_refusal(
            &pr.form,
            e,
            &pr.eqs,
            &pr.where_,
            workbooks,
            completion(pr, &prepared),
        )));
    }
    // Pass 2: every row's total (and group points) checked; the versions built.
    let mut versions = Vec::new();
    let mut key_sheet_rows = Vec::new();
    let mut infos = Vec::new();
    for pr in prepared {
        let (points, source) = pr.derived.expect("refusals returned above");
        let (form, idx, key) = (pr.form, pr.idx, pr.key);
        let lead = &workbooks[idx[0]];
        let mut checked = 0usize;
        let mut reproduced = 0usize;
        let mut group_checked = 0usize;
        let mut failures = Vec::new();
        for &k in &idx {
            let w = &workbooks[k];
            for r in &w.rows {
                checked += 1;
                let sum: Exact = (0..n)
                    .filter(|q| earns(&key, *q, &r.answers[*q]))
                    .map(|q| points[q])
                    .sum();
                if sum == r.total {
                    reproduced += 1;
                } else {
                    failures.push(format!(
                        "{} row {}: Remark {}, points give {}",
                        w.file,
                        r.row,
                        exact_text(&r.total),
                        exact_text(&sum)
                    ));
                }
                for (g, gp) in w.groups.iter().zip(&r.group_points) {
                    let Some(gp) = gp else { continue };
                    group_checked += 1;
                    let s: Exact = g
                        .questions
                        .iter()
                        .map(|q| usize::from(*q) - 1)
                        .filter(|q| earns(&key, *q, &r.answers[*q]))
                        .map(|q| points[q])
                        .sum();
                    if s != *gp {
                        failures.push(format!(
                            "{} row {} group {}: Remark {}, points give {}",
                            w.file,
                            r.row,
                            g.label,
                            exact_text(gp),
                            exact_text(&s)
                        ));
                    }
                }
            }
        }
        let sum: Exact = points.iter().sum();
        if sum != lead.total_possible {
            failures.push(format!(
                "{}: the points sum to {}, the Answer Key row says {}",
                lead.file,
                exact_text(&sum),
                exact_text(&lead.total_possible)
            ));
        }
        if !failures.is_empty() {
            return Err(AppError::Input(format!(
                "version {form}: the points ({}) do not reproduce {} Remark total(s) exactly: \
                 {}; give the points with --points FILE, or check the grading in Remark and \
                 export again",
                source.describe(),
                failures.len(),
                failures
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let mut students = 0;
        let mut ks = 0;
        for &k in &idx {
            let w = &workbooks[k];
            let mut s = 0;
            let mut kk = 0;
            for r in &w.rows {
                match r.key_sheet {
                    None => s += 1,
                    Some(kind) => {
                        kk += 1;
                        key_sheet_rows.push(KeySheetRowInfo {
                            file: w.file.clone(),
                            row: r.row,
                            masked_id: masked(&r.student_id),
                            kind,
                            answers_equal_key: (0..n).all(|q| earns(&key, q, &r.answers[q])),
                            full_marks: r.total == w.total_possible,
                        });
                    }
                }
            }
            students += s;
            ks += kk;
            infos.push(WorkbookInfo {
                file: w.file.clone(),
                sha384: w.sha384.clone(),
                form: form.clone(),
                students: s,
                key_sheets: kk,
                footer_rows: w.footer_rows,
                has_section: w.has_section,
                has_gender: w.has_gender,
                groups: w.groups.len(),
            });
        }
        versions.push(ImportedVersion {
            form: form.clone(),
            remark_version: version_of[idx[0]]
                .as_ref()
                .map(|v| v.1.clone())
                .unwrap_or_default(),
            workbooks: idx.iter().map(|k| workbooks[*k].file.clone()).collect(),
            key: lead.key.clone(),
            points,
            points_source: source,
            total_possible: lead.total_possible,
            groups: lead.groups.clone(),
            students,
            key_sheet_rows: ks,
            totals_reproduced: reproduced,
            rows_checked: checked,
            group_points_checked: group_checked,
        });
    }
    // The same student in several workbooks.
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for w in workbooks {
        for r in w.rows.iter().filter(|r| r.key_sheet.is_none()) {
            if !r.student_id.is_empty() {
                *ids.entry(r.student_id.as_str()).or_default() += 1;
            }
        }
    }
    let dup = ids.values().filter(|c| **c > 1).count();
    if dup > 0 {
        notes.push(format!(
            "{dup} student ID(s) appear in more than one row (another section or version, or \
             a sheet scanned twice in Remark)"
        ));
    }
    let blank = workbooks
        .iter()
        .flat_map(|w| &w.rows)
        .filter(|r| r.student_id.is_empty())
        .count();
    if blank > 0 {
        notes.push(format!("{blank} row(s) have no student ID"));
    }
    let lost = workbooks
        .iter()
        .flat_map(|w| &w.rows)
        .filter(|r| id_lost_digits(&r.student_id))
        .count();
    if lost > 0 {
        notes.push(format!(
            "{lost} row(s) have a student ID in scientific notation that lost digits (Excel \
             turned it into a number such as 2.21E+09): they cannot be matched by ID; export the \
             workbook again from Remark, or format the ID column as text before saving it"
        ));
    }
    // Learning objectives that leave questions out, or share them.
    for v in &versions {
        if v.groups.is_empty() {
            continue;
        }
        let mut count = vec![0usize; n];
        for g in &v.groups {
            for q in &g.questions {
                count[usize::from(*q) - 1] += 1;
            }
        }
        let none: Vec<usize> = (0..n).filter(|q| count[*q] == 0).collect();
        let many: Vec<usize> = (0..n).filter(|q| count[*q] > 1).collect();
        if !none.is_empty() {
            notes.push(format!(
                "version {}: questions {} are in no learning objective: no CLO mapping is \
                 imported for them",
                v.form,
                list(&none)
            ));
        }
        if !many.is_empty() {
            notes.push(format!(
                "version {}: questions {} are in more than one learning objective: each is \
                 mapped to every one of them (weight 1)",
                v.form,
                list(&many)
            ));
        }
    }
    // Each objective's points and questions per version: Remark's learning objectives are
    // groups of printed positions, so versions in another order may not describe the same
    // items under one name (§3.5: imported, needs review).
    if versions.iter().any(|v| !v.groups.is_empty()) {
        let per: Vec<(String, ObjectiveTotals)> = versions
            .iter()
            .map(|v| (v.form.clone(), objective_totals(v)))
            .collect();
        let names: BTreeSet<&String> = per.iter().flat_map(|(_, m)| m.keys()).collect();
        let mut differ = Vec::new();
        for name in names {
            let vals: Vec<String> = per
                .iter()
                .map(|(f, m)| match m.get(name) {
                    Some((p, k)) => {
                        format!("{} points over {k} questions in version {f}", exact_text(p))
                    }
                    None => format!("none in version {f}"),
                })
                .collect();
            let first = per.first().and_then(|(_, m)| m.get(name));
            if per.len() > 1 && per.iter().any(|(_, m)| m.get(name) != first) {
                differ.push(format!("{name}: {}", vals.join(", ")));
            }
        }
        if !differ.is_empty() {
            notes.push(format!(
                "the learning objectives differ between the versions ({}): Remark groups printed \
                 positions, so one objective may not describe the same items in every version; \
                 check each version's rows in clo.csv before approving them",
                differ.join("; ")
            ));
        }
        notes.push(
            "the CLO domain is not in Remark's export: every imported CLO is written as domain K \
             (knowledge and understanding); correct the domain column of clo.csv"
                .into(),
        );
    }
    Ok(RemarkImport {
        questions,
        versions,
        workbooks: infos,
        key_sheet_rows,
        notes,
    })
}

/// A version's learning objectives by name: their points and number of questions.
type ObjectiveTotals = BTreeMap<String, (Exact, usize)>;

/// Each learning objective of a version (by name): its points and number of questions.
fn objective_totals(v: &ImportedVersion) -> ObjectiveTotals {
    let mut m: BTreeMap<String, (Exact, usize)> = BTreeMap::new();
    for g in &v.groups {
        let e = m
            .entry(g.name.clone())
            .or_insert((Exact::from_integer(0), 0));
        e.0 += g.possible;
        e.1 += g.questions.len();
    }
    m
}

/// The workbooks of one version must give the same key, total possible and learning
/// objectives, else the import is refused naming the files.
fn agree(form: &str, idx: &[usize], workbooks: &[RemarkWorkbook]) -> Result<(), AppError> {
    let lead = &workbooks[idx[0]];
    let n = lead.questions;
    for &k in &idx[1..] {
        let w = &workbooks[k];
        if w.key != lead.key {
            let qs: Vec<usize> = (0..n).filter(|q| w.key[*q] != lead.key[*q]).collect();
            return Err(AppError::Input(format!(
                "version {form}: the Answer Key rows of {} and {} differ (questions {}); the \
                 workbooks must come from one exam graded with one key: export them again after \
                 correcting the key in Remark, or import them separately",
                lead.file,
                w.file,
                list(&qs)
            )));
        }
        if w.total_possible != lead.total_possible {
            return Err(AppError::Input(format!(
                "version {form}: {} gives {} total possible points and {} gives {}; the \
                 workbooks must come from one exam graded alike",
                lead.file,
                exact_text(&lead.total_possible),
                w.file,
                exact_text(&w.total_possible)
            )));
        }
        if w.groups != lead.groups {
            return Err(AppError::Input(format!(
                "version {form}: the learning objectives of {} ({}) and {} ({}) differ; the \
                 workbooks must come from one exam graded alike",
                lead.file,
                group_list(&lead.groups),
                w.file,
                group_list(&w.groups)
            )));
        }
    }
    Ok(())
}

/// Why the points of a version cannot be imported, and what to do (R10). `hint`: another
/// version's points left for the undetermined questions when the versions are the same
/// questions in another order.
fn describe_refusal(
    form: &str,
    e: &PointsError,
    eqs: &[Equation],
    where_: &[(usize, u32)],
    workbooks: &[RemarkWorkbook],
    hint: Option<(String, Vec<Exact>)>,
) -> String {
    const GIVE: &str = "give the points with --points FILE (`question,points` or \
        `question,points_A,points_B`; a question left empty is solved from the totals)";
    match e {
        PointsError::Underdetermined { questions, .. } => {
            let mut hints = Vec::new();
            let students = eqs.len().saturating_sub(1);
            for &q in questions {
                // Minus the total-possible equation, which counts every question.
                let c = eqs
                    .iter()
                    .filter(|e| e.questions.contains(&q))
                    .count()
                    .saturating_sub(1);
                if c == 0 {
                    hints.push(format!("no student answered question {} correctly", q + 1));
                } else if c == students {
                    hints.push(format!(
                        "every student answered question {} correctly",
                        q + 1
                    ));
                }
            }
            let mut same: BTreeMap<Vec<usize>, Vec<usize>> = BTreeMap::new();
            for &q in questions {
                let pattern: Vec<usize> = eqs
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.questions.contains(&q))
                    .map(|(i, _)| i)
                    .collect();
                same.entry(pattern).or_default().push(q);
            }
            for qs in same.values().filter(|v| v.len() > 1) {
                hints.push(format!(
                    "questions {} were answered correctly by exactly the same students, so only \
                     their sum is known",
                    list(qs)
                ));
            }
            // The learning objectives that hold these questions give only their sum.
            let lead = where_.last().map(|(k, _)| &workbooks[*k]);
            for g in lead.map(|w| w.groups.as_slice()).unwrap_or_default() {
                let inside: Vec<usize> = questions
                    .iter()
                    .copied()
                    .filter(|q| g.questions.contains(&(*q as u16 + 1)))
                    .collect();
                if inside.is_empty() || g.questions.len() < 2 {
                    continue;
                }
                let split = g.possible / Exact::from_integer(g.questions.len() as i128);
                hints.push(format!(
                    "the learning objective `{}` gives {} points for its {} questions; split \
                     equally that is {} each, a guess the import does not make: give the points \
                     of questions {} if that is how they were graded",
                    g.label,
                    exact_text(&g.possible),
                    g.questions.len(),
                    exact_text(&split),
                    list(&inside)
                ));
            }
            let mut m = format!(
                "version {form}: the Remark totals{} do not determine the points of questions \
                 {}{}; {GIVE}",
                if lead.is_some_and(|w| !w.groups.is_empty()) {
                    " and learning-objective points"
                } else {
                    ""
                },
                list(questions),
                if hints.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", hints.join("; "))
                }
            );
            if let Some((other, left)) = hint {
                let vals: BTreeSet<String> = left.iter().map(exact_text).collect();
                m.push_str(&format!(
                    ". If version {form} is version {other}'s questions in another order, the \
                     points version {other} leaves for them are {}",
                    left.iter().map(exact_text).collect::<Vec<_>>().join(", ")
                ));
                if vals.len() == 1 {
                    let v = vals.into_iter().next().unwrap_or_default();
                    m.push_str(&format!(
                        ": {v} each, e.g. a points file `question,points_{form}` with the rows {}",
                        questions
                            .iter()
                            .map(|q| format!("`{},{v}`", q + 1))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                } else {
                    m.push_str(" (which question carries which is not in the export)");
                }
            }
            m
        }
        PointsError::Inconsistent { equations } => {
            let rows: Vec<String> = equations
                .iter()
                .filter_map(|e| where_.get(*e))
                .map(|(k, r)| {
                    if *r == 0 {
                        format!("{} Answer Key total possible", workbooks[*k].file)
                    } else {
                        format!("{} row {r}", workbooks[*k].file)
                    }
                })
                .take(8)
                .collect();
            format!(
                "version {form}: no points per question reproduce every Remark total exactly{} \
                 (Remark may have used partial credit, extra credit or a total edited by hand); \
                 {GIVE}, or check the grading settings in Remark and export again",
                if rows.is_empty() {
                    String::new()
                } else {
                    format!(" (not reproduced: {})", rows.join(", "))
                }
            )
        }
        other => format!("version {form}: {other}; {GIVE}"),
    }
}

fn group_list(g: &[LoGroup]) -> String {
    if g.is_empty() {
        "none".into()
    } else {
        g.iter()
            .map(|g| format!("{} {}", g.label, exact_text(&g.possible)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl RemarkImport {
    /// Form letters in order.
    pub fn forms(&self) -> Vec<String> {
        self.versions.iter().map(|v| v.form.clone()).collect()
    }

    /// The version of form `f`.
    pub fn version(&self, f: &str) -> Option<&ImportedVersion> {
        self.versions.iter().find(|v| v.form == f)
    }

    /// The keys as the exam definition takes them.
    pub fn form_keys(&self) -> Vec<FormKey> {
        self.versions
            .iter()
            .map(|v| FormKey {
                form: v.form.clone(),
                answers: v.key.clone(),
            })
            .collect()
    }

    /// The points as a key file gives them (one column when every version agrees).
    pub fn key_points(&self) -> KeyFilePoints {
        let text = |v: &ImportedVersion| v.points.iter().map(exact_text).collect::<Vec<_>>();
        let all: Vec<Vec<String>> = self.versions.iter().map(text).collect();
        if all.iter().all(|p| *p == all[0]) {
            KeyFilePoints {
                all: all.first().cloned(),
                per_form: BTreeMap::new(),
            }
        } else {
            KeyFilePoints {
                all: None,
                per_form: self
                    .versions
                    .iter()
                    .map(|v| (v.form.clone(), text(v)))
                    .collect(),
            }
        }
    }

    /// The exact points of form `f`'s questions.
    pub fn points_of(&self, f: &str) -> Option<&[Exact]> {
        self.version(f).map(|v| v.points.as_slice())
    }

    /// `key.csv` in the `--key` layout: `question,A,B,points` (or `points_A,points_B` when the
    /// versions carry different points), exact values.
    pub fn key_csv(&self) -> String {
        let p = self.key_points();
        let mut out = String::from("question");
        for v in &self.versions {
            out.push(',');
            out.push_str(&v.form);
        }
        if p.per_form.is_empty() {
            out.push_str(",points");
        } else {
            for v in &self.versions {
                out.push_str(",points_");
                out.push_str(&v.form);
            }
        }
        out.push_str("\r\n");
        for q in 0..usize::from(self.questions) {
            out.push_str(&(q + 1).to_string());
            for v in &self.versions {
                out.push(',');
                out.push_str(&v.key[q]);
            }
            match &p.all {
                Some(all) => {
                    out.push(',');
                    out.push_str(&all[q]);
                }
                None => {
                    for v in &self.versions {
                        out.push(',');
                        out.push_str(&exact_text(&v.points[q]));
                    }
                }
            }
            out.push_str("\r\n");
        }
        out
    }

    /// The learning objectives as item → CLO mappings (§3.5, §11.2): weight 1, flagged
    /// "imported, needs review"; one row per version when the versions' groups differ.
    /// `None` without learning objectives.
    pub fn outcome_setup(&self) -> Option<OutcomeSetup> {
        if self.versions.iter().all(|v| v.groups.is_empty()) {
            return None;
        }
        let same = self
            .versions
            .iter()
            .all(|v| v.groups == self.versions[0].groups);
        let mut setup = OutcomeSetup {
            clos: Vec::new(),
            mapping: Vec::new(),
            split: false,
            policy: PolicyChoice::Default,
        };
        for v in &self.versions {
            for g in &v.groups {
                if !setup.clos.iter().any(|c| c.code == g.name) {
                    setup.clos.push(CloDefinition {
                        code: g.name.clone(),
                        title: format!("Remark learning objective {}", g.name),
                        title_ar: String::new(),
                        domain: DomainLetter::K,
                        plo: String::new(),
                    });
                }
                let (pts, count) = objective_totals(v)
                    .get(&g.name)
                    .cloned()
                    .unwrap_or_default();
                for q in &g.questions {
                    let form = if same { String::new() } else { v.form.clone() };
                    let m = CloMapping {
                        question: *q,
                        clo: g.name.clone(),
                        weight: "1".into(),
                        note: format!(
                            "Remark learning objective {}; {}{}: {} points over {} questions",
                            g.label,
                            g.name,
                            if same {
                                String::new()
                            } else {
                                format!(" in version {}", v.form)
                            },
                            exact_text(&pts),
                            count
                        ),
                        form,
                        imported: IMPORTED_SOURCE.into(),
                        approved_by: String::new(),
                    };
                    if !setup.mapping.contains(&m) {
                        setup.mapping.push(m);
                    }
                }
            }
            if same {
                break;
            }
        }
        Some(setup)
    }

    /// `clo.csv` in the `--clo` layout with the `form`, `status` and `source` columns; `None`
    /// without learning objectives. The domain is not in Remark's export: every CLO is written
    /// as K, to be checked with the mapping.
    pub fn clo_csv(&self) -> Option<String> {
        let o = self.outcome_setup()?;
        let mut out = String::from("clo,domain,title,questions,weight,note,form,status,source\r\n");
        // One row per (CLO, version, note) with its question list.
        let mut rows: BTreeMap<(usize, String, String), Vec<u16>> = BTreeMap::new();
        for m in &o.mapping {
            let ci = o.clos.iter().position(|c| c.code == m.clo).unwrap_or(0);
            rows.entry((ci, m.form.clone(), m.note.clone()))
                .or_default()
                .push(m.question);
        }
        for ((ci, form, note), qs) in rows {
            let c = &o.clos[ci];
            out.push_str(&format!(
                "{},K,{},\"{}\",1,\"{}\",{},\"{}\",{}\r\n",
                csv_text(&c.code),
                csv_text(&c.title),
                ranges(&qs),
                note.replace('"', "\"\""),
                form,
                IMPORTED_STATUS,
                IMPORTED_SOURCE
            ));
        }
        Some(out)
    }

    /// `version_map.csv`: which workbook gives which version, Remark's version text, the
    /// rows, the points source and the total possible. Remark's export carries no
    /// cross-version item map: each version is scored in its own printed order.
    pub fn version_map_csv(&self) -> String {
        let mut out = String::from(
            "form,remark_version,workbook,sha384,students,key_sheet_rows,footer_rows,\
             learning_objectives,total_possible,points_source\r\n",
        );
        for w in &self.workbooks {
            let v = self.version(&w.form);
            out.push_str(&format!(
                "{},{},\"{}\",{},{},{},{},{},{},\"{}\"\r\n",
                w.form,
                csv_text(&v.map(|v| v.remark_version.clone()).unwrap_or_default()),
                w.file.replace('"', "\"\""),
                w.sha384,
                w.students,
                w.key_sheets,
                w.footer_rows,
                w.groups,
                v.map(|v| exact_text(&v.total_possible)).unwrap_or_default(),
                v.map(|v| v.points_source.describe()).unwrap_or_default()
            ));
        }
        out
    }

    /// The summary shown before anything is written: counts, sources and checks only (no
    /// student ID in full, no name, no key letter).
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "Remark import: {} workbook(s), {} version(s) ({}), {} questions\n",
            self.workbooks.len(),
            self.versions.len(),
            self.forms().join(", "),
            self.questions
        ));
        for w in &self.workbooks {
            s.push_str(&format!(
                "  {}: version {}, {} student row(s), {} key-sheet row(s), {} footer row(s) \
                 skipped{}{}\n",
                w.file,
                w.form,
                w.students,
                w.key_sheets,
                w.footer_rows,
                if w.has_section {
                    ""
                } else {
                    ", no Section No column"
                },
                if w.groups > 0 {
                    format!(", {} learning-objective group(s)", w.groups)
                } else {
                    String::new()
                }
            ));
        }
        for v in &self.versions {
            let distinct: BTreeSet<String> = v.points.iter().map(exact_text).collect();
            s.push_str(&format!(
                "  version {}: points {}; total {} ({} distinct value(s): {}); Remark \
                 totals reproduced exactly: {} of {} row(s){}\n",
                v.form,
                v.points_source.describe(),
                exact_text(&v.total_possible),
                distinct.len(),
                distinct.into_iter().collect::<Vec<_>>().join(", "),
                v.totals_reproduced,
                v.rows_checked,
                if v.group_points_checked > 0 {
                    format!(
                        ", learning-objective points {} of {}",
                        v.group_points_checked, v.group_points_checked
                    )
                } else {
                    String::new()
                }
            ));
            if !v.groups.is_empty() {
                s.push_str(&format!(
                    "    learning objectives (CLO mappings, weight 1, {IMPORTED_STATUS}): {}\n",
                    group_list(&v.groups)
                ));
            }
        }
        if !self.key_sheet_rows.is_empty() {
            s.push_str(&format!(
                "  {} row(s) are key sheets graded as students (reported, never imported as \
                 students):\n",
                self.key_sheet_rows.len()
            ));
            for k in &self.key_sheet_rows {
                s.push_str(&format!(
                    "    {} row {}: ID {}{}{}\n",
                    k.file,
                    k.row,
                    k.masked_id,
                    match k.kind {
                        KeySheetKind::RepeatedDigitId => " (one repeated digit)",
                        KeySheetKind::AnswersEqualKey =>
                            " (answers identical to the key with full marks: a key sheet, or a \
                             student with every answer right; check)",
                    },
                    if k.kind == KeySheetKind::RepeatedDigitId
                        && k.answers_equal_key
                        && k.full_marks
                    {
                        ", answers equal the key, full marks"
                    } else {
                        ""
                    }
                ));
            }
        }
        for n in &self.notes {
            s.push_str(&format!("  note: {n}\n"));
        }
        s
    }

    /// Writes `key.csv`, `clo.csv` (with learning objectives) and `version_map.csv` into `dir`
    /// (created private; sync and network folders refused, SYS-015; files owner-only: the key
    /// is exam-confidential), with `manifest.json` (§16.3: the classification label, every
    /// file's SHA-384, the source workbooks' SHA-384, signed with this computer's key).
    /// Returns the paths written.
    pub fn write(&self, dir: &Path) -> Result<Vec<PathBuf>, AppError> {
        crate::keys::export_dir(dir)?;
        let mut files = vec![("key.csv", self.key_csv())];
        if let Some(c) = self.clo_csv() {
            files.push(("clo.csv", c));
        }
        files.push(("version_map.csv", self.version_map_csv()));
        let mut out = Vec::new();
        let mut listed = Vec::new();
        for (name, text) in &files {
            let p = dir.join(name);
            crate::keys::write_private(&p, text.as_bytes())?;
            listed.push(serde_json::json!({
                "path": name,
                "sha384": omr_store::jcs::sha384_hex(text.as_bytes()),
                "bytes": text.len(),
            }));
            out.push(p);
        }
        let manifest = serde_json::json!({
            "format": "omr-remark-import/1",
            "classification": "restricted",
            "contents": "the exam's answer key and points per version (exam-confidential), \
                         the learning objectives as CLO mappings (imported, needs review) and \
                         the version map; no student data",
            "purpose": "remark-import",
            "pii": false,
            "created_utc": crate::pipeline::now_utc(),
            "sources": self.workbooks.iter().map(|w| serde_json::json!({
                "file": w.file, "sha384": w.sha384,
            })).collect::<Vec<_>>(),
            "files": listed,
        });
        let canonical = omr_store::jcs::value_to_jcs(&manifest)
            .map_err(|e| AppError::Internal(format!("manifest: {e}")))?;
        let signed = crate::export::sign_detached(canonical.as_bytes())?;
        let p = dir.join("manifest.json");
        crate::keys::write_private(&p, &signed)?;
        out.push(p);
        Ok(out)
    }
}

fn csv_text(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// `1-8,11,13-14` from sorted 1-based questions.
fn ranges(qs: &[u16]) -> String {
    let mut v: Vec<u16> = qs.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        out.push(if i == j {
            v[i].to_string()
        } else {
            format!("{}-{}", v[i], v[j])
        });
        i = j + 1;
    }
    out.join(",")
}

/// Reads the workbooks and imports them ([`read_workbook_files`], [`import`]); `points` is an
/// optional points file ([`exam::read_points_file`]).
pub fn import_files(
    paths: &[PathBuf],
    points: Option<&Path>,
) -> Result<(Vec<RemarkWorkbook>, RemarkImport), AppError> {
    let wbs = read_workbook_files(paths)?;
    let given = match points {
        Some(p) => {
            let bytes = std::fs::read(p).map_err(|e| {
                AppError::Input(format!("{}: {e}; check the path of the file", p.display()))
            })?;
            let q = u16::try_from(wbs.first().map_or(0, |w| w.questions)).unwrap_or(u16::MAX);
            exam::read_points_file(&bytes, q)?
        }
        None => GivenPoints::new(),
    };
    let imp = import(&wbs, &given)?;
    Ok((wbs, imp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_groups_parse() {
        assert_eq!(RemarkAnswer::parse(" b "), RemarkAnswer::Option(1));
        assert_eq!(RemarkAnswer::parse(""), RemarkAnswer::Blank);
        assert_eq!(RemarkAnswer::parse("AC"), RemarkAnswer::Multi(vec![0, 2]));
        assert_eq!(RemarkAnswer::parse("C,A"), RemarkAnswer::Multi(vec![0, 2]));
        assert_eq!(RemarkAnswer::parse("?"), RemarkAnswer::Other("?".into()));
        assert_eq!(RemarkAnswer::parse("AC").text(), "MULTI:AC");
        assert_eq!(
            parse_group("Q1 (1-8)", 28),
            Some(Ok(("Q1".into(), (1..=8).collect())))
        );
        assert_eq!(
            parse_group("LO 2 (11-12, 20)", 28),
            Some(Ok(("LO 2".into(), vec![11, 12, 20])))
        );
        // A group naming a question the export does not have is another test's: refused.
        assert!(matches!(parse_group("Q1 (1-40)", 28), Some(Err(_))));
        assert!(matches!(parse_group("Q1 (8-1)", 28), Some(Err(_))));
        assert_eq!(parse_group("Overall", 28), None);
        assert_eq!(parse_group("Percent (%)", 28), None);
        assert!(id_lost_digits("2.21035E+09"));
        assert!(!id_lost_digits("2.210345601E+09"));
        assert!(!id_lost_digits("2210345601"));
        assert!(!id_lost_digits("ERROR 12345"));
        assert_eq!(ranges(&[1, 2, 3, 5, 7, 8]), "1-3,5,7-8");
        assert_eq!(form_of("A"), Some("A".into()));
        assert_eq!(form_of("2"), Some("B".into()));
        assert_eq!(form_of("Version b"), Some("B".into()));
        assert_eq!(form_of("AB"), None);
    }
}
