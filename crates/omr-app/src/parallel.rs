//! The parallel run with the incumbent (spec §23.7, §3.5): a graded exam beside Remark Office
//! OMR's graded export of the same sheets ([`crate::remark`]), for the instructor to adjudicate
//! every difference before release.
//!
//! - **Matching.** Booklets are matched to Remark rows by student ID (a number cell that lost
//!   its leading zeros matches too) when the answers agree as two readings of one sheet do (at
//!   least half identical, and half of those where either departs from the key): a booklet
//!   that carries another student's number is not paired with that student's row while its
//!   own row is there. The booklets and rows left over are paired by their answers (≥ 80 %
//!   identical, a clear best on both sides, the same version), and each such pairing is
//!   reported as an identity difference, never merged silently; an ID match whose answers
//!   differ is paired last, and reported. A workbook none of whose rows matches a booklet by
//!   ID is another section's (not scanned here): it is listed, not compared. When no row of
//!   any workbook pairs with a booklet, the workbooks are refused (another exam's).
//! - **Per question**: Remark's answer, ours and our status (auto-accepted, confirmed or
//!   changed by a reviewer, pending review); every difference with the bubble-row crop for
//!   adjudication.
//! - **Per student**: the weighted totals with the points imported from the workbooks (or
//!   given), Remark's total, and letter grades on the Saudi scale; a provisional total and
//!   the pending questions that would make it Remark's.
//! - **Identity differences**: a booklet with no row in Remark, a Remark row with no booklet,
//!   a different ID (paired by answers), Remark's ID equal to the handwritten ID when it is
//!   known, duplicate IDs.
//! - **Key sheets graded as students** in Remark ([`crate::remark::KeySheetKind`]) and their
//!   effect on the mean, top score and reliability Remark reported.
//!
//! The report is **Restricted** (student numbers and answers) and its head-to-head figures
//! stay inside the university (spec §3.5, Q-08): every file carries the banner, it is written
//! only where the user asks (sync and network folders refused, SYS-015; owner-only files; a
//! signed manifest) and the export is recorded in the audit log.

use crate::AppError;
use crate::app::ExamProject;
use crate::crops::Crop;
use crate::keysheet::{LABEL_LEAD_PITCHES, crop_svg, esc, rectified_row};
use crate::model::{Booklet, KeyCell, ResponseStatus};
use crate::remark::{
    GivenPoints, KeySheetKind, RemarkAnswer, RemarkImport, RemarkStats, RemarkWorkbook, earns,
};
use omr_model::Response;
use omr_report::{Cell, Row, Sheet};
use omr_scoring::{Exact, GradeOutcome, GradeScale, LetterBasis, RoundingPolicy, exact_text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The banner on every file of the report.
pub const RESTRICTED_BANNER: &str = "RESTRICTED (مقيد) - parallel run with Remark Office OMR: \
    student numbers and answers (PDPL personal data). Head-to-head figures stay inside the \
    university and leave it only after the licence review (spec §3.5, Q-08)";

/// The administrator's record of the licence review of parallel runs with the incumbent
/// (spec §3.5, Q-08: the university's legal affairs, due 2026-12-01): `licence_review.json` in
/// the private key folder ([`crate::keys::key_dir`]), `{"q08": "cleared" | "refused", "by":
/// "…", "date": "YYYY-MM-DD"}`. Without it the review is open: a comparison is an internal
/// check and says so; `refused` stops the comparison.
pub const LICENCE_REVIEW_FILE: &str = "licence_review.json";

/// The recorded Q-08 decision ([`LICENCE_REVIEW_FILE`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenceReview {
    /// `cleared` or `refused`.
    pub q08: String,
    /// Who recorded the decision of the legal review.
    #[serde(default)]
    pub by: String,
    /// When.
    #[serde(default)]
    pub date: String,
}

impl LicenceReview {
    /// The record in the private key folder, if any (an unreadable record counts as none).
    pub fn recorded() -> Option<Self> {
        let p = crate::keys::key_dir()?.join(LICENCE_REVIEW_FILE);
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    }

    /// The comparison's licence line: what Q-08 permits, in words.
    pub fn describe(r: Option<&Self>) -> String {
        match r {
            Some(r) if r.q08.eq_ignore_ascii_case("cleared") => format!(
                "Licence review (Q-08): cleared ({}{}); head-to-head figures still leave the \
                 university only after legal review",
                if r.by.is_empty() { "recorded" } else { &r.by },
                if r.date.is_empty() {
                    String::new()
                } else {
                    format!(", {}", r.date)
                }
            ),
            _ => "Licence review (Q-08): not recorded, so open (the university's legal affairs, \
                  due 2026-12-01): this comparison is an internal check, and its figures stay \
                  inside the university"
                .to_string(),
        }
    }
}

/// Least share of identical answers for pairing a booklet and a Remark row by their answers.
pub const PAIR_BY_ANSWERS_MIN: f64 = 0.8;

/// Questions where either reading departs from the key from which an ID match must also agree
/// on half of them (fewer say nothing about whose sheet it is).
const SAME_SHEET_MIN_INFORMATIVE: usize = 4;

/// A handwritten student number, when it is known (read by a person or another channel).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandwrittenId {
    /// The scan's file name (or a distinctive part of it).
    pub file: String,
    /// Page in the file (1-based).
    pub page: u32,
    /// The number(s) written by hand.
    pub ids: Vec<String>,
}

/// Options of the comparison.
#[derive(Clone, Debug, Default)]
pub struct CompareOptions {
    /// Points given by the instructor (else derived from the workbooks).
    pub given_points: GivenPoints,
    /// Handwritten numbers, when known.
    pub handwritten: Vec<HandwrittenId>,
    /// Bubble-row crops of the disagreements (read from the stored originals).
    pub crops: bool,
}

/// How a booklet and a Remark row were paired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchHow {
    /// The same student ID.
    StudentId,
    /// Remark's ID agrees with every digit we read confidently; ours is incomplete (the
    /// columns left blank or unclear wait in identity review, IDN-004).
    StudentIdPartial {
        /// Our grid read (`?` unclear, `_` blank).
        read: String,
    },
    /// The answers (identical answers of the questions compared).
    Answers {
        /// Identical answers.
        agree: usize,
        /// Questions compared.
        of: usize,
    },
}

/// A booklet paired with a Remark row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    /// Booklet index.
    pub booklet: usize,
    /// Workbook index.
    pub workbook: usize,
    /// Row index in the workbook.
    pub row: usize,
    /// How.
    pub how: MatchHow,
}

/// One question of a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agreement {
    /// Our settled answer is Remark's.
    Agree,
    /// Ours is pending review; the proposal is Remark's.
    AgreePending,
    /// Our settled answer differs.
    Differ,
    /// Ours is pending review; the proposal differs.
    DifferPending,
}

/// One question of a pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionRow {
    /// Pair index.
    pub pair: usize,
    /// Question (1-based).
    pub question: u16,
    /// Remark's answer.
    pub remark: String,
    /// Ours (`?` prefix: a proposal pending review).
    pub ours: String,
    /// Our status: `auto`, `confirmed`, `changed`, `pending`.
    pub status: String,
    /// Agreement.
    pub agreement: Agreement,
}

/// One booklet's totals beside Remark's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudentRow {
    /// Booklet index.
    pub booklet: usize,
    /// Booklet label with its file and page.
    pub label: String,
    /// Our student ID (accepted or proposed; `""` none).
    pub our_id: String,
    /// Remark's (`""` without a row).
    pub remark_id: String,
    /// Version (form letter).
    pub form: String,
    /// How it was paired (`""` unpaired).
    pub how: String,
    /// The exam graded the booklet (it has a row in the exam's score run): `ours`, `max` and
    /// `grade_ours` are the figures the exam releases. Otherwise (no key, a key not yet
    /// confirmed, no version) they are the answers-only check below.
    pub graded: bool,
    /// The exam's total for the booklet (its score run: the exam's points, voids and policy;
    /// settled questions only, a pending question is never credited).
    pub ours: Exact,
    /// The exam's maximum for the booklet.
    pub max: Exact,
    /// Every question settled.
    pub settled: bool,
    /// Questions pending review (1-based).
    pub pending: Vec<u16>,
    /// The exam's total with every pending question resolved as Remark read it.
    pub ours_if_remark: Exact,
    /// Answers only (a secondary check of the readings, not a grade): Remark's key and
    /// imported points applied to our answers, each pending question as Remark read it.
    pub answers_only: Exact,
    /// Remark's total.
    pub remark: Option<Exact>,
    /// Remark's total possible for the booklet's version.
    pub remark_possible: Option<Exact>,
    /// Our letter (Saudi scale) for our total.
    pub grade_ours: String,
    /// The Saudi letter for Remark's total, and Remark's own `Grade` cell.
    pub grade_remark: String,
}

/// Kinds of identity differences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IdentityKind {
    /// A booklet with no row in Remark.
    NoRemarkRow,
    /// A Remark row with no booklet (in a workbook whose other rows were scanned here).
    NoBooklet,
    /// Paired by answers under different IDs.
    DifferentId,
    /// Paired by answers; Remark lists the handwritten number (an operator correction).
    RemarkIdIsHandwritten,
    /// Paired by answers; Remark's row has no student number (e.g. `ERROR …`: Remark could
    /// not read the bubbled ID).
    RemarkIdNotRead,
    /// The same student ID, but the answers are another sheet's: paired by ID only because
    /// neither has a better match (another student's number bubbled, or a sheet of another
    /// test).
    SameIdAnswersDiffer,
    /// One ID on several booklets or rows.
    DuplicateId,
}

impl IdentityKind {
    /// Display text.
    pub fn describe(self) -> &'static str {
        match self {
            IdentityKind::NoRemarkRow => "booklet with no row in Remark",
            IdentityKind::NoBooklet => "Remark row with no booklet",
            IdentityKind::DifferentId => "different student ID (paired by answers)",
            IdentityKind::RemarkIdIsHandwritten => {
                "Remark lists the handwritten ID (paired by answers)"
            }
            IdentityKind::RemarkIdNotRead => {
                "Remark's row has no student number (paired by answers)"
            }
            IdentityKind::SameIdAnswersDiffer => {
                "same student ID, answers of another sheet (paired by ID, no better match)"
            }
            IdentityKind::DuplicateId => "one student ID on several booklets or rows",
        }
    }
}

/// An identity difference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityDiff {
    /// Kind.
    pub kind: IdentityKind,
    /// Booklet label and page (`""` none).
    pub booklet: String,
    /// Our ID (`""` none) and the grid as read.
    pub our_id: String,
    /// Our identity findings (IDN codes with sub-reasons).
    pub our_flags: Vec<String>,
    /// Workbook and row (`""` none).
    pub remark_row: String,
    /// Remark's ID.
    pub remark_id: String,
    /// The handwritten number(s), when known.
    pub handwritten: Vec<String>,
    /// What was found.
    pub detail: String,
}

/// Class statistics as Remark computes them (population SD; KR-20 on number right; α on the
/// weighted item scores).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassStats {
    /// Rows.
    pub n: usize,
    /// Mean total.
    pub mean: Option<f64>,
    /// Standard deviation (÷ N).
    pub sd: Option<f64>,
    /// Top total.
    pub max: Option<f64>,
    /// Top total in % of the total possible.
    pub max_percent: Option<f64>,
    /// KR-20.
    pub kr20: Option<f64>,
    /// Cronbach α.
    pub alpha: Option<f64>,
}

/// A workbook's key-sheet rows graded as students and their effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySheetEffect {
    /// Workbook.
    pub workbook: String,
    /// Key sheets: rows whose student ID is one repeated digit (worksheet row, masked ID,
    /// why); no student has such a number.
    pub rows: Vec<(u32, String, KeySheetKind)>,
    /// Rows whose answers are the key with full marks under a student-like ID and that no
    /// booklet here pairs with: a key sheet, or a student who answered every question right
    /// (worksheet row, masked ID). Never counted as key sheets.
    pub possible: Vec<(u32, String)>,
    /// What Remark reported (Test Statistics Report).
    pub reported: Option<RemarkStats>,
    /// Recomputed from every row, key sheets included (as Remark did).
    pub with: ClassStats,
    /// Recomputed without the key-sheet rows.
    pub without: ClassStats,
    /// Recomputed without the key sheets and the possible key sheets (when there are any).
    pub without_possible: Option<ClassStats>,
}

/// A question to adjudicate: a difference, or a question pending review here (Remark's
/// reading shown beside ours).
#[derive(Clone, Debug, PartialEq)]
pub struct Disagreement {
    /// Pair index.
    pub pair: usize,
    /// Differ, DifferPending or AgreePending.
    pub agreement: Agreement,
    /// Question (1-based).
    pub question: u16,
    /// Remark's answer.
    pub remark: String,
    /// Remark's answer as read from the workbook.
    pub remark_answer: RemarkAnswer,
    /// The workbook and row it comes from.
    pub remark_row: String,
    /// Ours.
    pub ours: String,
    /// Our status.
    pub status: String,
    /// Booklet label, file and page.
    pub page: String,
    /// The student.
    pub student: String,
    /// Our recognition's findings on the question (codes).
    pub findings: String,
    /// The bubble-row crop (with the printed question number).
    pub crop: Option<Crop>,
}

/// A workbook in the comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkbookScope {
    /// File.
    pub file: String,
    /// Version.
    pub form: String,
    /// Rows (key sheets included).
    pub rows: usize,
    /// Rows taken for key sheets.
    pub key_sheets: usize,
    /// Rows paired with booklets.
    pub paired: usize,
    /// Some row matches a booklet by ID: this exam's scans include the workbook's sheets.
    pub in_scope: bool,
}

/// Aggregate counts (the head-to-head figures: Restricted, inside the university).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    /// Booklets of the exam.
    pub booklets: usize,
    /// Remark rows (key sheets included).
    pub remark_rows: usize,
    /// Rows in workbooks not scanned here.
    pub out_of_scope_rows: usize,
    /// Key-sheet rows graded as students (an ID of one repeated digit).
    pub key_sheet_rows: usize,
    /// Rows whose answers equal the key with full marks under a student-like ID and no
    /// booklet: a key sheet or a perfect score (never counted as key sheets).
    pub possible_key_sheet_rows: usize,
    /// Pairs by student ID.
    pub paired_by_id: usize,
    /// Pairs by student ID where our read is incomplete (Remark's ID agrees with every digit
    /// read confidently).
    pub paired_by_partial_id: usize,
    /// Pairs by answers.
    pub paired_by_answers: usize,
    /// Questions compared.
    pub questions: usize,
    /// Settled and equal.
    pub agree: usize,
    /// Pending, proposal equal.
    pub agree_pending: usize,
    /// Settled and different.
    pub differ: usize,
    /// Pending, proposal different.
    pub differ_pending: usize,
    /// Paired booklets the exam graded and fully settled (the totals are the exam's score
    /// run: its own points, voids and policy).
    pub totals_settled: usize,
    /// … whose total and maximum equal Remark's total and total possible.
    pub totals_settled_equal: usize,
    /// Paired booklets the exam graded with questions pending.
    pub totals_provisional: usize,
    /// … equal to Remark's once each pending question is resolved as Remark read it.
    pub totals_provisional_equal_if_remark: usize,
    /// Paired booklets the exam has not graded (no key or no version): compared on the
    /// answers only.
    pub totals_not_graded: usize,
    /// Answers only (Remark's key and imported points applied to our answers, pending
    /// questions as Remark read them): paired booklets whose total equals Remark's. A check of
    /// the readings, not of the grades.
    pub answers_only_equal: usize,
    /// Learning-objective subtotals compared (§3.5 item 6, CLO results): per paired, settled
    /// booklet and objective of its workbook, the exam's points on the objective's questions
    /// (its score run) beside Remark's `points / possible` cell.
    pub objective_subtotals: usize,
    /// … equal to Remark's.
    pub objective_subtotals_equal: usize,
    /// Questions whose points in the exam differ from Remark's (imported) points.
    pub points_differences: usize,
    /// Versions whose maximum in the exam differs from Remark's total possible: no total can
    /// agree there.
    pub maximum_differences: usize,
    /// Settled differences from an answer the exam auto-accepted that are not in the review
    /// queue: the report alone does not hold release for them
    /// ([`ExamProject::queue_parallel_differences`] queues them).
    pub not_queued: usize,
    /// Parallel-run differences waiting in the review queue (MRK-001
    /// `parallel_run_disagreement`, open): each holds release until it is decided.
    pub held_in_review: usize,
    /// Rows whose Remark total the imported points reproduce.
    pub remark_totals_reproduced: usize,
    /// Identity differences by kind.
    pub identity: BTreeMap<String, usize>,
    /// Questions where Remark's key differs from ours.
    pub key_differences: usize,
}

/// The parallel run.
#[derive(Clone, Debug, PartialEq)]
pub struct ParallelRun {
    /// Exam title and course.
    pub exam: String,
    /// Forms.
    pub forms: Vec<String>,
    /// Questions.
    pub questions: u16,
    /// The import (key, points, key sheets) the comparison used.
    pub import: RemarkImport,
    /// Workbooks.
    pub workbooks: Vec<WorkbookScope>,
    /// Remark's key vs ours: (form, question, Remark, ours).
    pub key_differences: Vec<(String, u16, String, String)>,
    /// Points that differ: (form, question, the exam's points or `void`, Remark's points).
    pub points_differences: Vec<(String, u16, String, String)>,
    /// Versions whose maximum differs: (form, the exam's maximum, Remark's total possible).
    pub maximum_differences: Vec<(String, String, String)>,
    /// Items queued for review by [`ExamProject::queue_parallel_differences`] after this
    /// comparison (`None`: not queued from it).
    pub queued: Option<usize>,
    /// What the licence review (Q-08) permits, as recorded ([`LicenceReview::describe`]).
    pub licence: String,
    /// Pairs.
    pub pairs: Vec<Pair>,
    /// Per question.
    pub questions_rows: Vec<QuestionRow>,
    /// Per booklet.
    pub students: Vec<StudentRow>,
    /// Identity differences.
    pub identity: Vec<IdentityDiff>,
    /// Key sheets graded as students.
    pub key_sheets: Vec<KeySheetEffect>,
    /// Differences to adjudicate (and questions pending review here), with crops.
    pub disagreements: Vec<Disagreement>,
    /// Counts.
    pub counts: Counts,
    /// The Remark rows of each pair (student ID, workbook, row) for the CSVs.
    remark_refs: Vec<(String, String, u32)>,
}

fn digits(s: &str) -> String {
    omr_identity::normalize_id(s)
        .chars()
        .filter(|c| !matches!(c, ' ' | '-'))
        .collect()
}

/// Our student ID of a booklet: the accepted or proposed student, else a fully read grid.
fn our_id(b: &Booklet) -> String {
    if let Some(s) = b.student() {
        return digits(s);
    }
    let r = digits(&b.identity.read);
    if !r.is_empty() && r.bytes().all(|c| c.is_ascii_digit()) {
        r
    } else {
        String::new()
    }
}

fn same_id(remark: &str, numeric: bool, ours: &str) -> bool {
    let r = digits(remark);
    if r.is_empty() || ours.is_empty() {
        return false;
    }
    if r == ours {
        return true;
    }
    // A number cell that lost its leading zeros.
    numeric && r.len() < ours.len() && format!("{r:0>w$}", w = ours.len()) == ours
}

/// Remark's ID agrees with every digit of our grid read that was read confidently (`?`
/// unclear and `_` blank columns match anything), and at least half of the columns were.
fn compatible(remark: &str, read: &str) -> bool {
    let r = digits(remark);
    let g: Vec<char> = read.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    if r.is_empty() || !r.bytes().all(|c| c.is_ascii_digit()) || r.len() != g.len() {
        return false;
    }
    let certain = g.iter().filter(|c| c.is_ascii_digit()).count();
    2 * certain >= g.len()
        && r.chars()
            .zip(&g)
            .all(|(a, b)| !b.is_ascii_digit() || a == *b)
}

/// A student number has digits only.
fn is_number(id: &str) -> bool {
    let d = digits(id);
    !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())
}

fn status_text(s: ResponseStatus) -> &'static str {
    match s {
        ResponseStatus::Auto => "auto",
        ResponseStatus::Pending => "pending",
        ResponseStatus::Confirmed => "confirmed",
        ResponseStatus::Changed => "changed",
    }
}

fn letter(scale: &GradeScale, x: &Exact, max: &Exact) -> String {
    if *max <= Exact::from_integer(0) {
        return String::new();
    }
    let pct = *x * Exact::from_integer(100) / *max;
    let rp = RoundingPolicy::default();
    let basis = match rp.letter_basis {
        LetterBasis::Rounded => rp.round(&pct),
        LetterBasis::Unrounded => pct,
    };
    GradeOutcome::letter_for(scale, &basis)
        .map(|g| g.code().to_string())
        .unwrap_or_default()
}

fn f2(x: &Exact) -> f64 {
    omr_scoring::num::to_f64(x)
}

/// Class statistics of `rows` as Remark computes them.
fn class_stats(
    rows: &[&crate::remark::RemarkRow],
    key: &[crate::exam::ParsedKey],
    points: &[Exact],
    possible: &Exact,
) -> ClassStats {
    let n = rows.len();
    let k = points.len();
    if n == 0 {
        return ClassStats::default();
    }
    let x: Vec<Vec<bool>> = rows
        .iter()
        .map(|r| (0..k).map(|q| earns(key, q, &r.answers[q])).collect())
        .collect();
    let totals: Vec<f64> = rows.iter().map(|r| f2(&r.total)).collect();
    let pvar = |v: &[f64]| {
        let m = v.iter().sum::<f64>() / v.len() as f64;
        v.iter().map(|a| (a - m).powi(2)).sum::<f64>() / v.len() as f64
    };
    let mean = totals.iter().sum::<f64>() / n as f64;
    let max = totals.iter().cloned().fold(f64::MIN, f64::max);
    let vt = pvar(&totals);
    let nr: Vec<f64> = x
        .iter()
        .map(|r| r.iter().filter(|b| **b).count() as f64)
        .collect();
    let vnr = pvar(&nr);
    let kf = k as f64;
    let spq: f64 = (0..k)
        .map(|q| {
            let p = x.iter().filter(|r| r[q]).count() as f64 / n as f64;
            p * (1.0 - p)
        })
        .sum();
    let kr20 = (k >= 2 && vnr > 1e-12).then(|| kf / (kf - 1.0) * (1.0 - spq / vnr));
    let item_var: f64 = (0..k)
        .map(|q| {
            let col: Vec<f64> = x
                .iter()
                .map(|r| if r[q] { f2(&points[q]) } else { 0.0 })
                .collect();
            pvar(&col)
        })
        .sum();
    let weighted: Vec<f64> = x
        .iter()
        .map(|r| (0..k).filter(|q| r[*q]).map(|q| f2(&points[q])).sum())
        .collect();
    let vw = pvar(&weighted);
    let alpha = (k >= 2 && vw > 1e-12).then(|| kf / (kf - 1.0) * (1.0 - item_var / vw));
    let poss = f2(possible);
    ClassStats {
        n,
        mean: Some(mean),
        sd: Some(vt.sqrt()),
        max: Some(max),
        max_percent: (poss > 0.0).then(|| 100.0 * max / poss),
        kr20,
        alpha,
    }
}

/// Compares a graded exam with Remark's graded workbooks of the same sheets.
pub fn compare(
    p: &ExamProject,
    workbooks: &[RemarkWorkbook],
    opts: &CompareOptions,
) -> Result<ParallelRun, AppError> {
    compare_under(p, workbooks, opts, LicenceReview::recorded())
}

/// [`compare`] under the licence review `licence` (the record of [`LicenceReview::recorded`]):
/// refused when the review does not permit parallel runs.
pub fn compare_under(
    p: &ExamProject,
    workbooks: &[RemarkWorkbook],
    opts: &CompareOptions,
    licence: Option<LicenceReview>,
) -> Result<ParallelRun, AppError> {
    if let Some(r) = licence
        .as_ref()
        .filter(|r| r.q08.eq_ignore_ascii_case("refused"))
    {
        return Err(AppError::Input(format!(
            "the licence review of parallel runs with Remark (Q-08) recorded{}{} does not permit \
             them: no comparison is made; ask the university's legal affairs, or remove {} from \
             the private key folder if the decision was recorded in error",
            if r.by.is_empty() {
                String::new()
            } else {
                format!(" by {}", r.by)
            },
            if r.date.is_empty() {
                String::new()
            } else {
                format!(" on {}", r.date)
            },
            LICENCE_REVIEW_FILE
        )));
    }
    let s = &p.session;
    let def = &s.definition;
    let n = usize::from(def.questions);
    if let Some(w) = workbooks.iter().find(|w| w.questions != n) {
        return Err(AppError::Input(format!(
            "{} has {} questions and the exam {n}: compare the workbooks of this exam",
            w.file, w.questions
        )));
    }
    let imp = crate::remark::import(workbooks, &opts.given_points)?;
    let forms = def.forms.clone();
    if let Some(v) = imp.versions.iter().find(|v| !forms.contains(&v.form)) {
        return Err(AppError::Input(format!(
            "the Remark workbooks give version {} and the exam has versions {}: compare the \
             workbooks of this exam",
            v.form,
            forms.join(", ")
        )));
    }
    let key_of_wb: Vec<String> = workbooks
        .iter()
        .map(|w| {
            imp.workbooks
                .iter()
                .find(|i| i.file == w.file && i.sha384 == w.sha384)
                .map(|i| i.form.clone())
                .unwrap_or_default()
        })
        .collect();
    let parsed: BTreeMap<String, Vec<crate::exam::ParsedKey>> = imp
        .versions
        .iter()
        .map(|v| {
            (
                v.form.clone(),
                v.key
                    .iter()
                    .map(|k| {
                        crate::exam::parse_key_entry(k, 10)
                            .unwrap_or(crate::exam::ParsedKey::Missing)
                    })
                    .collect(),
            )
        })
        .collect();
    // Our key (when scored) vs Remark's.
    let our_key = s.key.as_ref().filter(|_| !s.key_unconfirmed());
    let mut key_differences = Vec::new();
    if let Some(k) = our_key {
        for v in &imp.versions {
            let f = forms.iter().position(|x| *x == v.form).expect("checked");
            for q in 0..n {
                let ours = k.forms.get(f).and_then(|c| c.get(q)).map(KeyCell::text);
                let theirs = crate::exam::parse_key_entry(&v.key[q], def.options)
                    .map(KeyCell::from)
                    .unwrap_or(KeyCell::Missing)
                    .text();
                if ours.as_deref() != Some(theirs.as_str()) {
                    key_differences.push((
                        v.form.clone(),
                        q as u16 + 1,
                        theirs,
                        ours.unwrap_or_default(),
                    ));
                }
            }
        }
    }
    // The exam's score run (its points, voids and policy: the figures it releases).
    let scored = p
        .scoring
        .as_ref()
        .and_then(|sc| sc.run.as_ref().ok().map(|r| (sc, r)));
    let exam_grades = scored.map(|(sc, _)| sc.grades()).unwrap_or_default();
    // The exam's points of question `q` on form `f`: `Some(None)` voided, `None` without a
    // key.
    let exam_points = |f: usize, q: usize| -> Option<Option<Exact>> {
        let k = our_key?;
        if matches!(k.forms.get(f).and_then(|c| c.get(q)), Some(KeyCell::Void)) {
            return Some(None);
        }
        if let Some((_, run)) = scored {
            let i = crate::score::master_item(k, n, f, q);
            return run
                .items
                .get(i)
                .map(|m| Some(omr_scoring::widen(&m.max_points)));
        }
        k.points_of(f)
            .get(q)
            .and_then(|t| omr_scoring::parse_points(t).ok())
            .map(|x| Some(omr_scoring::widen(&x)))
    };
    // The exam's points vs Remark's, and each version's maximum vs Remark's total possible.
    let mut points_differences = Vec::new();
    let mut maximum_differences = Vec::new();
    if our_key.is_some() {
        for v in &imp.versions {
            let f = forms.iter().position(|x| *x == v.form).expect("checked");
            let mut exam_max = Exact::from_integer(0);
            for q in 0..n {
                let theirs = v.points[q];
                match exam_points(f, q) {
                    Some(Some(x)) => {
                        exam_max += x;
                        if x != theirs {
                            points_differences.push((
                                v.form.clone(),
                                q as u16 + 1,
                                exact_text(&x),
                                exact_text(&theirs),
                            ));
                        }
                    }
                    Some(None) => points_differences.push((
                        v.form.clone(),
                        q as u16 + 1,
                        "void".to_string(),
                        exact_text(&theirs),
                    )),
                    None => {}
                }
            }
            if exam_max != v.total_possible {
                maximum_differences.push((
                    v.form.clone(),
                    exact_text(&exam_max),
                    exact_text(&v.total_possible),
                ));
            }
        }
    }
    // --- Matching by student ID.
    let ours: Vec<String> = s.booklets.iter().map(our_id).collect();
    let mut rows: Vec<(usize, usize)> = Vec::new(); // (workbook, row) not taken for an ID-less key sheet
    for (w, wb) in workbooks.iter().enumerate() {
        for (r, row) in wb.rows.iter().enumerate() {
            if row.key_sheet != Some(KeySheetKind::RepeatedDigitId) {
                rows.push((w, r));
            }
        }
    }
    let row_of = |w: usize, r: usize| &workbooks[w].rows[r];
    let answers_agree = |bi: usize, w: usize, r: usize| -> usize {
        let b = &s.booklets[bi];
        let rr = row_of(w, r);
        b.questions
            .iter()
            .zip(&rr.answers)
            .filter(|(q, a)| a.same_as(&q.current))
            .count()
    };
    // Identical answers among the questions where either reading departs from the key, and
    // how many such questions: two students agree on most questions by being right; their
    // wrong answers tell them apart.
    let informative = |b: usize, w: usize, r: usize| -> (usize, usize) {
        let Some(key) = parsed.get(&key_of_wb[w]) else {
            return (0, 0);
        };
        let row = row_of(w, r);
        let mut both = (0usize, 0usize);
        for (q, qs) in s.booklets[b].questions.iter().enumerate() {
            let theirs_right = earns(key, q, &row.answers[q]);
            let ours_right = matches!(&qs.current, Response::Single(o)
                if earns(key, q, &RemarkAnswer::Option(*o)));
            if !(theirs_right && ours_right) {
                both.1 += 1;
                both.0 += usize::from(row.answers[q].same_as(&qs.current));
            }
        }
        both
    };
    // Two readings of one sheet: at least half the answers identical, and at least half of
    // those where either departs from the key (when there are a few such questions).
    let plausible = |b: usize, w: usize, r: usize| -> bool {
        let (ia, io) = informative(b, w, r);
        2 * answers_agree(b, w, r) >= n && (io < SAME_SHEET_MIN_INFORMATIVE || 2 * ia >= io)
    };
    // ID matches whose answers are another sheet's: paired last, when nothing better is found.
    let mut weak: Vec<(usize, (usize, usize))> = Vec::new();
    let mut pairs: Vec<Pair> = Vec::new();
    let mut used_b = vec![false; s.booklets.len()];
    let mut used_r: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut identity: Vec<IdentityDiff> = Vec::new();
    let label = |bi: usize| -> String {
        let b = &s.booklets[bi];
        let page = b
            .pages
            .first()
            .and_then(|&pg| s.pages.get(pg))
            .map(|pg| {
                format!(
                    "{} page {}",
                    s.sources
                        .get(pg.source)
                        .map(|x| x
                            .name
                            .split(" (unreadable: ")
                            .next()
                            .unwrap_or("")
                            .to_string())
                        .unwrap_or_default(),
                    pg.index + 1
                )
            })
            .unwrap_or_default();
        format!("{} ({page})", b.label())
    };
    let flags = |bi: usize| -> Vec<String> {
        let b = &s.booklets[bi];
        b.identity
            .exceptions
            .iter()
            .chain(b.exceptions.iter().filter(|e| e.code.starts_with("IDN")))
            .map(|e| match &e.sub_reason {
                Some(r) => format!("{} {r}", e.code),
                None => e.code.clone(),
            })
            .collect()
    };
    let rowname = |w: usize, r: usize| format!("{} row {}", workbooks[w].file, row_of(w, r).row);
    // ID → booklets and rows.
    /// A row as (workbook, row index).
    type RowRef = (usize, usize);
    let mut by_id: BTreeMap<String, (Vec<usize>, Vec<RowRef>)> = BTreeMap::new();
    for (bi, id) in ours.iter().enumerate() {
        if !id.is_empty() {
            by_id.entry(id.clone()).or_default().0.push(bi);
        }
    }
    for &(w, r) in &rows {
        let row = row_of(w, r);
        if row.student_id.is_empty() {
            continue;
        }
        let id = digits(&row.student_id);
        let key = if by_id.contains_key(&id) {
            id
        } else {
            // A number cell that lost its leading zeros matches a booklet's padded ID.
            by_id
                .keys()
                .find(|k| same_id(&row.student_id, row.numeric_id, k))
                .cloned()
                .unwrap_or(id)
        };
        by_id.entry(key).or_default().1.push((w, r));
    }
    for (id, (bs, rs)) in &by_id {
        if bs.is_empty() || rs.is_empty() {
            continue;
        }
        if bs.len() > 1 || rs.len() > 1 {
            identity.push(IdentityDiff {
                kind: IdentityKind::DuplicateId,
                booklet: bs.iter().map(|b| label(*b)).collect::<Vec<_>>().join("; "),
                our_id: id.clone(),
                our_flags: bs.iter().flat_map(|b| flags(*b)).collect(),
                remark_row: rs
                    .iter()
                    .map(|(w, r)| rowname(*w, *r))
                    .collect::<Vec<_>>()
                    .join("; "),
                remark_id: id.clone(),
                handwritten: Vec::new(),
                detail: format!(
                    "{} booklet(s) and {} Remark row(s) with this ID: paired by their answers",
                    bs.len(),
                    rs.len()
                ),
            });
        }
        // Greedy by identical answers.
        let mut cand: Vec<(usize, usize, (usize, usize))> = Vec::new();
        for &b in bs {
            for &(w, r) in rs {
                cand.push((answers_agree(b, w, r), b, (w, r)));
            }
        }
        cand.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (_, b, (w, r)) in cand {
            if used_b[b] || used_r.contains(&(w, r)) {
                continue;
            }
            if !plausible(b, w, r) {
                weak.push((b, (w, r)));
                continue;
            }
            used_b[b] = true;
            used_r.insert((w, r));
            pairs.push(Pair {
                booklet: b,
                workbook: w,
                row: r,
                how: MatchHow::StudentId,
            });
        }
    }
    // Scope: workbooks with an ID match.
    let in_scope: Vec<bool> = (0..workbooks.len())
        .map(|w| pairs.iter().any(|p| p.workbook == w))
        .collect();
    // --- Our incomplete reads that Remark's ID completes (unique on both sides).
    let mut partial: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    let mut partial_rows: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for b in (0..s.booklets.len()).filter(|b| !used_b[*b]) {
        let read = &s.booklets[b].identity.read;
        for &(w, r) in rows
            .iter()
            .filter(|(w, r)| in_scope[*w] && !used_r.contains(&(*w, *r)))
        {
            if compatible(&row_of(w, r).student_id, read) && plausible(b, w, r) {
                partial.entry(b).or_default().push((w, r));
                partial_rows.entry((w, r)).or_default().push(b);
            }
        }
    }
    for (b, rs) in &partial {
        if let [(w, r)] = rs.as_slice()
            && partial_rows[&(*w, *r)].len() == 1
        {
            used_b[*b] = true;
            used_r.insert((*w, *r));
            pairs.push(Pair {
                booklet: *b,
                workbook: *w,
                row: *r,
                how: MatchHow::StudentIdPartial {
                    read: s.booklets[*b].identity.read.clone(),
                },
            });
        }
    }
    // --- Pairing the rest by answers: the same version, ≥ 80 % identical answers, ≥ 80 %
    // identical where either departs from the key (two strong students agree on most
    // questions by being right; their wrong answers tell them apart), a clear best.
    let left_b: Vec<usize> = (0..s.booklets.len()).filter(|b| !used_b[*b]).collect();
    let left_r: Vec<(usize, usize)> = rows
        .iter()
        .copied()
        .filter(|(w, r)| in_scope[*w] && !used_r.contains(&(*w, *r)))
        .collect();
    let form_of_b = |b: usize| s.booklets[b].form.map(|f| forms[f].clone());
    let mut cand: Vec<(usize, usize, (usize, usize))> = Vec::new();
    for &b in &left_b {
        for &(w, r) in &left_r {
            if let Some(f) = form_of_b(b)
                && !key_of_wb[w].is_empty()
                && key_of_wb[w] != f
            {
                continue;
            }
            let a = answers_agree(b, w, r);
            let (ia, io) = informative(b, w, r);
            if a as f64 >= PAIR_BY_ANSWERS_MIN * n as f64
                && io >= 2
                && ia as f64 >= PAIR_BY_ANSWERS_MIN * io as f64
            {
                cand.push((a, b, (w, r)));
            }
        }
    }
    cand.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let second_best = |b: usize, wr: (usize, usize), cand: &[(usize, usize, (usize, usize))]| {
        cand.iter()
            .filter(|c| (c.1 == b) != (c.2 == wr))
            .map(|c| c.0)
            .max()
            .unwrap_or(0)
    };
    let all_cand = cand.clone();
    for (a, b, (w, r)) in cand {
        if used_b[b] || used_r.contains(&(w, r)) {
            continue;
        }
        // A clear best on both sides: two more identical answers than any alternative.
        if second_best(b, (w, r), &all_cand) + 2 > a {
            continue;
        }
        used_b[b] = true;
        used_r.insert((w, r));
        pairs.push(Pair {
            booklet: b,
            workbook: w,
            row: r,
            how: MatchHow::Answers { agree: a, of: n },
        });
        let row = row_of(w, r);
        let bl = &s.booklets[b];
        let pg = bl.pages.first().and_then(|&pg| s.pages.get(pg));
        let hand: Vec<String> = pg
            .and_then(|pg| {
                let name = s.sources.get(pg.source).map(|x| x.name.as_str())?;
                opts.handwritten
                    .iter()
                    .find(|h| name.contains(&h.file) && h.page == pg.index + 1)
            })
            .map(|h| h.ids.iter().map(|i| digits(i)).collect())
            .unwrap_or_default();
        let rid = digits(&row.student_id);
        let number = is_number(&row.student_id);
        let is_hand = number && hand.contains(&rid);
        let oid = &ours[b];
        let detail = {
            let mut d = format!("{a} of {n} answers identical; ");
            if !number && crate::remark::id_lost_digits(&row.student_id) {
                d.push_str(&format!(
                    "Remark's ID cell `{}` is a number in scientific notation that lost digits \
                     (Excel), not a student number; ",
                    row.student_id
                ));
            } else if !number {
                d.push_str(&format!(
                    "Remark's ID cell `{}` is not a student number (Remark could not read the \
                     bubbled ID); ",
                    row.student_id
                ));
            }
            if oid.is_empty() {
                d.push_str(&format!(
                    "our grid read `{}` (no complete student number)",
                    bl.identity.read
                ));
            } else if !number {
                d.push_str("ours reads it in full");
            } else if rid.len() != oid.len() {
                d.push_str(&format!(
                    "Remark's ID has {} digits, ours {} (grid read `{}`)",
                    rid.len(),
                    oid.len(),
                    bl.identity.read
                ));
            } else {
                let dd = rid.chars().zip(oid.chars()).filter(|(x, y)| x != y).count();
                d.push_str(&format!("the IDs differ in {dd} digit(s)"));
            }
            if is_hand {
                d.push_str("; Remark's ID is the number written by hand (an operator correction)");
            } else if !hand.is_empty() && number {
                d.push_str("; the handwritten number is neither");
            }
            d
        };
        identity.push(IdentityDiff {
            kind: if !number {
                IdentityKind::RemarkIdNotRead
            } else if is_hand {
                IdentityKind::RemarkIdIsHandwritten
            } else {
                IdentityKind::DifferentId
            },
            booklet: label(b),
            our_id: oid.clone(),
            our_flags: flags(b),
            remark_row: rowname(w, r),
            remark_id: row.student_id.clone(),
            handwritten: hand,
            detail,
        });
    }
    // Nothing pairs as two readings of one sheet: these are not this exam's workbooks.
    if pairs.is_empty() && !s.booklets.is_empty() && !rows.is_empty() {
        return Err(AppError::Input(format!(
            "none of the {} booklets matches a Remark row as a reading of the same sheet ({} \
             share a student ID with a row whose answers are another sheet's): these workbooks \
             are not of this exam's sheets; compare the workbooks Remark graded from them",
            s.booklets.len(),
            weak.iter().map(|w| w.0).collect::<BTreeSet<_>>().len()
        )));
    }
    // ID matches whose answers differ, when neither side found a better match.
    for (b, (w, r)) in weak {
        if used_b[b] || used_r.contains(&(w, r)) {
            continue;
        }
        used_b[b] = true;
        used_r.insert((w, r));
        pairs.push(Pair {
            booklet: b,
            workbook: w,
            row: r,
            how: MatchHow::StudentId,
        });
        let (ia, io) = informative(b, w, r);
        identity.push(IdentityDiff {
            kind: IdentityKind::SameIdAnswersDiffer,
            booklet: label(b),
            our_id: ours[b].clone(),
            our_flags: flags(b),
            remark_row: rowname(w, r),
            remark_id: row_of(w, r).student_id.clone(),
            handwritten: Vec::new(),
            detail: format!(
                "{} of {n} answers identical, {ia} of {io} where either departs from the key: \
                 another student's sheet under this number (a bubbled ID that is another \
                 student's), or a sheet of another test; compared, but check the identity first",
                answers_agree(b, w, r)
            ),
        });
    }
    for b in (0..s.booklets.len()).filter(|b| !used_b[*b]) {
        identity.push(IdentityDiff {
            kind: IdentityKind::NoRemarkRow,
            booklet: label(b),
            our_id: ours[b].clone(),
            our_flags: flags(b),
            remark_row: String::new(),
            remark_id: String::new(),
            handwritten: Vec::new(),
            detail: format!(
                "no Remark row with this ID, and none whose answers match (grid read `{}`)",
                s.booklets[b].identity.read
            ),
        });
    }
    for &(w, r) in &rows {
        let row = row_of(w, r);
        if !in_scope[w] || used_r.contains(&(w, r)) || row.key_sheet.is_some() {
            continue;
        }
        identity.push(IdentityDiff {
            kind: IdentityKind::NoBooklet,
            booklet: String::new(),
            our_id: String::new(),
            our_flags: Vec::new(),
            remark_row: rowname(w, r),
            remark_id: row.student_id.clone(),
            handwritten: Vec::new(),
            detail: "no booklet with this ID, and none whose answers match: a sheet Remark read \
                     that is not among this exam's scans, or one we could not attribute"
                .into(),
        });
    }
    pairs.sort_by_key(|p| p.booklet);
    // --- Per question and per student.
    let labels = def.option_labels();
    let scale = GradeScale::saudi();
    let mut questions_rows = Vec::new();
    let mut disagreements = Vec::new();
    let mut counts = Counts {
        booklets: s.booklets.len(),
        ..Counts::default()
    };
    let mut images: BTreeMap<usize, Option<std::sync::Arc<omr_imaging::GrayImage>>> =
        BTreeMap::new();
    let pair_of: BTreeMap<usize, usize> = pairs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.booklet, i))
        .collect();
    let mut students = Vec::new();
    let mut remark_refs = Vec::new();
    for (bi, b) in s.booklets.iter().enumerate() {
        let form = b.form.map(|f| forms[f].clone()).unwrap_or_default();
        let pi = pair_of.get(&bi).copied();
        let row = pi.map(|i| row_of(pairs[i].workbook, pairs[i].row));
        let version = if form.is_empty() {
            pi.map(|i| key_of_wb[pairs[i].workbook].clone())
                .unwrap_or_default()
        } else {
            form.clone()
        };
        let pts = imp.points_of(&version).map(<[Exact]>::to_vec);
        let f_idx = forms.iter().position(|x| *x == version);
        // Our key for the booklet's version (else Remark's).
        let key_cell = |q: usize| -> KeyCell {
            match (our_key, f_idx) {
                (Some(k), Some(f)) => k.forms[f][q].clone(),
                _ => parsed
                    .get(&version)
                    .and_then(|v| v.get(q))
                    .cloned()
                    .map(KeyCell::from)
                    .unwrap_or(KeyCell::Missing),
            }
        };
        let credited = |q: usize, r: &Response| -> bool {
            matches!((key_cell(q), r), (KeyCell::Options(v), Response::Single(o)) if v.contains(o))
        };
        let remark_pts = |q: usize| {
            pts.as_ref()
                .and_then(|v| v.get(q).copied())
                .unwrap_or_else(|| Exact::from_integer(1))
        };
        // The answers-only check: Remark's key and points on our answers (pending questions as
        // Remark read them); and the questions pending review here.
        let remark_key = parsed.get(&version);
        let (mut answers_only, mut answers_max) = (Exact::from_integer(0), Exact::from_integer(0));
        let mut pending = Vec::new();
        for (q, qs) in b.questions.iter().enumerate() {
            let held = b.hold.is_some() || qs.status == ResponseStatus::Pending;
            if held && key_cell(q) != KeyCell::Void {
                pending.push(q as u16 + 1);
            }
            let Some(rk) = remark_key else { continue };
            answers_max += remark_pts(q);
            let read = match (held, row) {
                (true, Some(r)) => r.answers[q].clone(),
                (true, None) => RemarkAnswer::Blank,
                (false, _) => match &qs.current {
                    Response::Single(o) => RemarkAnswer::Option(*o),
                    _ => RemarkAnswer::Blank,
                },
            };
            if earns(rk, q, &read) {
                answers_only += remark_pts(q);
            }
        }
        // The exam's own figures: its score run's row for the booklet.
        let exam_row = scored.and_then(|(sc, run)| {
            let r = sc.row_of.get(bi).copied().flatten()?;
            Some((r, run.students.get(r)?))
        });
        let graded = exam_row.is_some();
        let (ours_t, max, settled, if_remark, grade_ours) = match exam_row {
            Some((r, res)) => {
                let settled = res.status == omr_scoring::AssessmentStatus::Scored;
                let mut if_remark = res.raw;
                if let (Some(row), Some(f)) = (row, f_idx) {
                    for q in pending.iter().map(|q| usize::from(*q) - 1) {
                        if let RemarkAnswer::Option(o) = &row.answers[q]
                            && credited(q, &Response::Single(*o))
                            && let Some(Some(x)) = exam_points(f, q)
                        {
                            if_remark += x;
                        }
                    }
                }
                let grade = if settled {
                    exam_grades
                        .get(r)
                        .cloned()
                        .flatten()
                        .map(|g| g.code().to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                (
                    res.raw,
                    res.max,
                    settled && pending.is_empty(),
                    if_remark,
                    grade,
                )
            }
            // Not graded by the exam: the answers-only figures, labelled as such.
            None => {
                let settled = pending.is_empty();
                let mut ours_settled = answers_only;
                if let (Some(rk), Some(r)) = (remark_key, row) {
                    for q in pending.iter().map(|q| usize::from(*q) - 1) {
                        if earns(rk, q, &r.answers[q]) {
                            ours_settled -= remark_pts(q);
                        }
                    }
                }
                let grade = if settled {
                    letter(&scale, &ours_settled, &answers_max)
                } else {
                    String::new()
                };
                (ours_settled, answers_max, settled, answers_only, grade)
            }
        };
        let (remark_id, how) = match (pi, row) {
            (Some(i), Some(r)) => (
                digits(&r.student_id),
                match &pairs[i].how {
                    MatchHow::StudentId => "student ID".to_string(),
                    MatchHow::StudentIdPartial { read } => {
                        format!("student ID (our read `{read}` incomplete)")
                    }
                    MatchHow::Answers { agree, of } => format!("answers ({agree}/{of})"),
                },
            ),
            _ => (String::new(), String::new()),
        };
        // The learning objectives (CLO results, §3.5 item 6): the exam's points on each
        // objective's questions beside Remark's subtotal, for a settled booklet.
        if let (Some((r, res)), Some(row), Some(pi), Some(k), Some(f)) =
            (exam_row, row, pi, our_key, f_idx)
            && res.status == omr_scoring::AssessmentStatus::Scored
            && let Some((_, run)) = scored
        {
            let wb = &workbooks[pairs[pi].workbook];
            for (g, gp) in wb.groups.iter().zip(&row.group_points) {
                let Some(gp) = gp else { continue };
                let ours_g: Exact = g
                    .questions
                    .iter()
                    .map(|q| {
                        let i = crate::score::master_item(k, n, f, usize::from(*q) - 1);
                        omr_scoring::widen(&run.item_score(r, i))
                    })
                    .sum();
                counts.objective_subtotals += 1;
                counts.objective_subtotals_equal += usize::from(ours_g == *gp);
            }
        }
        let rem_total = row.map(|r| r.total);
        let possible = imp.version(&version).map(|v| v.total_possible);
        if let Some(rt) = rem_total {
            counts.answers_only_equal += usize::from(answers_only == rt);
            if !graded {
                counts.totals_not_graded += 1;
            } else if settled {
                counts.totals_settled += 1;
                counts.totals_settled_equal +=
                    usize::from(ours_t == rt && possible.is_none_or(|x| x == max));
            } else {
                counts.totals_provisional += 1;
                counts.totals_provisional_equal_if_remark +=
                    usize::from(if_remark == rt && possible.is_none_or(|x| x == max));
            }
        }
        students.push(StudentRow {
            booklet: bi,
            label: label(bi),
            our_id: ours[bi].clone(),
            remark_id,
            form: version.clone(),
            how,
            graded,
            ours: ours_t,
            max,
            settled,
            pending,
            ours_if_remark: if_remark,
            answers_only,
            remark: rem_total,
            remark_possible: possible,
            grade_ours,
            grade_remark: row
                .map(|r| {
                    let l = letter(&scale, &r.total, &possible.unwrap_or(max));
                    if r.grade.is_empty() {
                        l
                    } else {
                        format!("{l} (Remark: {})", r.grade)
                    }
                })
                .unwrap_or_default(),
        });
        let (Some(i), Some(r)) = (pi, row) else {
            continue;
        };
        remark_refs.push((
            digits(&r.student_id),
            workbooks[pairs[i].workbook].file.clone(),
            r.row,
        ));
        for (q, qs) in b.questions.iter().enumerate() {
            let pend = b.hold.is_some() || qs.status == ResponseStatus::Pending;
            let same = r.answers[q].same_as(&qs.current);
            let agreement = match (pend, same) {
                (false, true) => Agreement::Agree,
                (true, true) => Agreement::AgreePending,
                (false, false) => Agreement::Differ,
                (true, false) => Agreement::DifferPending,
            };
            counts.questions += 1;
            match agreement {
                Agreement::Agree => counts.agree += 1,
                Agreement::AgreePending => counts.agree_pending += 1,
                Agreement::Differ => counts.differ += 1,
                Agreement::DifferPending => counts.differ_pending += 1,
            }
            let ours_text = crate::export::response_text(&qs.current, &labels);
            let status = if b.hold.is_some() {
                "pending"
            } else {
                status_text(qs.status)
            };
            let ours_shown = if pend {
                format!("?{ours_text}")
            } else {
                ours_text.clone()
            };
            questions_rows.push(QuestionRow {
                pair: i,
                question: q as u16 + 1,
                remark: r.answers[q].text(),
                ours: ours_shown.clone(),
                status: status.into(),
                agreement,
            });
            if agreement != Agreement::Agree {
                let crop = if opts.crops {
                    let img = images
                        .entry(qs.page)
                        .or_insert_with(|| p.page_image(qs.page).ok())
                        .clone();
                    let geom = s.pages.get(qs.page).and_then(|pg| {
                        let rd = pg.read.as_ref()?;
                        let k = rd
                            .questions
                            .iter()
                            .position(|x| x.question == qs.recognized.question)?;
                        rd.question_geom.get(k)
                    });
                    let marked: Vec<u8> = match &qs.current {
                        Response::Single(o) => vec![*o],
                        Response::Multi(v) | Response::Set(v) => v.clone(),
                        _ => Vec::new(),
                    };
                    img.zip(geom).and_then(|(img, g)| {
                        rectified_row(&img, g, &labels, &marked, 2.0, LABEL_LEAD_PITCHES)
                    })
                } else {
                    None
                };
                disagreements.push(Disagreement {
                    pair: i,
                    agreement,
                    question: q as u16 + 1,
                    remark: r.answers[q].text(),
                    remark_answer: r.answers[q].clone(),
                    remark_row: rowname(pairs[i].workbook, pairs[i].row),
                    ours: ours_shown,
                    status: status.into(),
                    page: label(bi),
                    student: ours[bi].clone(),
                    findings: qs
                        .recognized
                        .exceptions
                        .iter()
                        .map(|e| match &e.sub_reason {
                            Some(x) => format!("{} {x}", e.code),
                            None => e.code.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    crop,
                });
            }
        }
    }
    // --- Key sheets graded as students, and what they did to Remark's statistics.
    let mut key_sheets = Vec::new();
    let mut key_sheet_rows = 0usize;
    let mut possible_key_sheet_rows = 0usize;
    for (w, wb) in workbooks.iter().enumerate() {
        // Key sheets: one repeated digit for an ID. Answers equal to the key with full marks
        // under a student-like ID and no booklet: possibly a key sheet, possibly a student
        // with every answer right (another section's, or absent here): never taken for one.
        let ks: Vec<usize> = wb
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.key_sheet == Some(KeySheetKind::RepeatedDigitId))
            .map(|(r, _)| r)
            .collect();
        let maybe: Vec<usize> = wb
            .rows
            .iter()
            .enumerate()
            .filter(|(r, row)| {
                row.key_sheet == Some(KeySheetKind::AnswersEqualKey) && !used_r.contains(&(w, *r))
            })
            .map(|(r, _)| r)
            .collect();
        if ks.is_empty() && maybe.is_empty() {
            continue;
        }
        key_sheet_rows += ks.len();
        possible_key_sheet_rows += maybe.len();
        let form = &key_of_wb[w];
        let (Some(v), Some(key)) = (imp.version(form), parsed.get(form)) else {
            continue;
        };
        let all: Vec<&crate::remark::RemarkRow> = wb.rows.iter().collect();
        let without: Vec<&crate::remark::RemarkRow> = wb
            .rows
            .iter()
            .enumerate()
            .filter(|(r, _)| !ks.contains(r))
            .map(|(_, x)| x)
            .collect();
        let without_possible: Vec<&crate::remark::RemarkRow> = wb
            .rows
            .iter()
            .enumerate()
            .filter(|(r, _)| !ks.contains(r) && !maybe.contains(r))
            .map(|(_, x)| x)
            .collect();
        key_sheets.push(KeySheetEffect {
            workbook: wb.file.clone(),
            rows: ks
                .iter()
                .map(|r| {
                    let row = &wb.rows[*r];
                    (
                        row.row,
                        crate::export::mask_numbers(&row.student_id),
                        KeySheetKind::RepeatedDigitId,
                    )
                })
                .collect(),
            possible: maybe
                .iter()
                .map(|r| {
                    let row = &wb.rows[*r];
                    (row.row, crate::export::mask_numbers(&row.student_id))
                })
                .collect(),
            reported: wb.stats.clone(),
            with: class_stats(&all, key, &v.points, &v.total_possible),
            without: class_stats(&without, key, &v.points, &v.total_possible),
            without_possible: (!maybe.is_empty())
                .then(|| class_stats(&without_possible, key, &v.points, &v.total_possible)),
        });
    }
    let workbook_scope: Vec<WorkbookScope> = workbooks
        .iter()
        .enumerate()
        .map(|(w, wb)| WorkbookScope {
            file: wb.file.clone(),
            form: key_of_wb[w].clone(),
            rows: wb.rows.len(),
            key_sheets: wb
                .rows
                .iter()
                .filter(|r| r.key_sheet == Some(KeySheetKind::RepeatedDigitId))
                .count(),
            paired: pairs.iter().filter(|p| p.workbook == w).count(),
            in_scope: in_scope[w],
        })
        .collect();
    counts.remark_rows = workbooks.iter().map(|w| w.rows.len()).sum();
    counts.out_of_scope_rows = workbook_scope
        .iter()
        .filter(|w| !w.in_scope)
        .map(|w| w.rows)
        .sum();
    counts.key_sheet_rows = key_sheet_rows;
    counts.possible_key_sheet_rows = possible_key_sheet_rows;
    counts.paired_by_id = pairs
        .iter()
        .filter(|p| p.how == MatchHow::StudentId)
        .count();
    counts.paired_by_partial_id = pairs
        .iter()
        .filter(|p| matches!(p.how, MatchHow::StudentIdPartial { .. }))
        .count();
    counts.paired_by_answers = pairs.len() - counts.paired_by_id - counts.paired_by_partial_id;
    counts.remark_totals_reproduced = imp.versions.iter().map(|v| v.totals_reproduced).sum();
    for d in &identity {
        *counts.identity.entry(format!("{:?}", d.kind)).or_default() += 1;
    }
    counts.key_differences = key_differences.len();
    counts.points_differences = points_differences.len();
    counts.maximum_differences = maximum_differences.len();
    counts.not_queued = disagreements
        .iter()
        .filter(|d| d.agreement == Agreement::Differ && d.status == "auto")
        .count();
    counts.held_in_review = s
        .review
        .items
        .iter()
        .filter(|i| i.is_open() && i.sub_reason.as_deref() == Some(PARALLEL_SUB))
        .count();
    identity.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.booklet.cmp(&b.booklet)));
    Ok(ParallelRun {
        exam: format!("{} {}", def.course_code, def.title)
            .trim()
            .to_string(),
        forms,
        questions: def.questions,
        import: imp,
        workbooks: workbook_scope,
        key_differences,
        points_differences,
        maximum_differences,
        queued: None,
        licence: LicenceReview::describe(licence.as_ref()),
        pairs,
        questions_rows,
        students,
        identity,
        key_sheets,
        disagreements,
        counts,
        remark_refs,
    })
}

fn opt(x: Option<f64>, d: usize) -> String {
    x.map(|v| format!("{v:.d$}"))
        .unwrap_or_else(|| "n/a".into())
}

fn banner_sheet(name: &str, title: &str, header: &[&str], rows: Vec<Row>) -> Sheet {
    Sheet {
        name: name.into(),
        title: format!("{RESTRICTED_BANNER} | {title}"),
        headers: vec![
            vec![RESTRICTED_BANNER.to_string()],
            header.iter().map(|h| h.to_string()).collect(),
        ],
        rows,
        notes: Vec::new(),
        rtl: false,
        widths: Vec::new(),
    }
}

/// The sub-reason of a settled parallel-run difference queued for review (MRK-001, §23.7).
pub const PARALLEL_SUB: &str = "parallel_run_disagreement";

impl StudentRow {
    /// The booklet's total and maximum equal Remark's total and total possible (settled), or
    /// will once each pending question is resolved as Remark read it.
    pub fn equals_remark(&self) -> bool {
        let max_ok = self.remark_possible.is_none_or(|x| x == self.max);
        match &self.remark {
            Some(r) if self.settled => *r == self.ours && max_ok,
            Some(r) => *r == self.ours_if_remark && max_ok,
            None => false,
        }
    }

    /// The comparison with Remark's total, in words.
    pub fn comparison(&self) -> String {
        let Some(r) = &self.remark else {
            return String::new();
        };
        let mut out = match self.remark_possible {
            Some(x) if x != self.max => format!(
                "maximum differs: the exam's {} vs Remark's {} (the points differ); ",
                exact_text(&self.max),
                exact_text(&x)
            ),
            _ => String::new(),
        };
        out.push_str(&if self.settled && *r == self.ours {
            "total equal".to_string()
        } else if self.settled {
            format!("total differs by {}", exact_text(&(self.ours - r)))
        } else if *r == self.ours_if_remark {
            "total equal once pending questions are resolved as Remark read them".into()
        } else {
            "total differs (provisional)".into()
        });
        if !self.graded {
            out.push_str(" (not graded by the exam: answers only)");
        }
        out
    }
}

impl ParallelRun {
    /// What holds release after this comparison: the queued review items, or nothing (the
    /// report alone).
    pub fn hold_text(&self) -> String {
        let c = &self.counts;
        let mut out = Vec::new();
        if c.not_queued > 0 {
            out.push(format!(
                "This report does not hold release for its {} settled difference(s) from an \
                 answer this exam auto-accepted: they are not in the review queue. To have each \
                 decided in Review before release (spec §23.7), queue them (omr compare remark \
                 --queue; in the desktop app, Queue for review).",
                c.not_queued
            ));
        }
        if c.held_in_review > 0 {
            out.push(format!(
                "{} parallel-run difference(s) wait in Review (MRK-001 {PARALLEL_SUB}){} and hold \
                 release until the instructor decides each against the bubble row; each decision \
                 is recorded as a field label (spec §23.7).",
                c.held_in_review,
                match self.queued {
                    Some(n) => format!(", {n} queued from this comparison"),
                    None => String::new(),
                }
            ));
        }
        if out.is_empty() {
            out.push(
                "No settled difference from an answer this exam auto-accepted is open: nothing \
                 to queue. Questions pending review here are decided in Review as usual."
                    .to_string(),
            );
        }
        out.join(" ")
    }

    /// Per-question CSV: every question of every pair.
    pub fn questions_csv(&self) -> Result<Vec<u8>, AppError> {
        let rows = self
            .questions_rows
            .iter()
            .map(|q| {
                let p = &self.pairs[q.pair];
                let (rid, wb, rn) = self.remark_refs.get(q.pair).cloned().unwrap_or_default();
                let st = self.students.iter().find(|s| s.booklet == p.booklet);
                Row::new(vec![
                    Cell::text(st.map(|s| s.label.clone()).unwrap_or_default()),
                    Cell::Id(st.map(|s| s.our_id.clone()).unwrap_or_default()),
                    Cell::Id(rid),
                    Cell::text(wb),
                    Cell::Int(i64::from(rn)),
                    Cell::text(st.map(|s| s.form.clone()).unwrap_or_default()),
                    Cell::Int(i64::from(q.question)),
                    Cell::text(q.remark.clone()),
                    Cell::text(q.ours.clone()),
                    Cell::text(q.status.clone()),
                    Cell::text(format!("{:?}", q.agreement)),
                ])
            })
            .collect();
        csv(&banner_sheet(
            "Questions",
            "per question",
            &[
                "booklet",
                "our_student_id",
                "remark_student_id",
                "remark_workbook",
                "remark_row",
                "version",
                "question",
                "remark",
                "ours",
                "our_status",
                "agreement",
            ],
            rows,
        ))
    }

    /// Per-student CSV: every booklet, with Remark's total when paired.
    pub fn students_csv(&self) -> Result<Vec<u8>, AppError> {
        let rows = self
            .students
            .iter()
            .map(|s| {
                Row::new(vec![
                    Cell::text(s.label.clone()),
                    Cell::Id(s.our_id.clone()),
                    Cell::Id(s.remark_id.clone()),
                    Cell::text(s.how.clone()),
                    Cell::text(s.form.clone()),
                    Cell::text(if s.graded {
                        "the exam's score run"
                    } else {
                        "not graded by the exam: answers only"
                    }),
                    Cell::num(Some(exact_text(&s.ours)), 2),
                    Cell::num(Some(exact_text(&s.max)), 2),
                    Cell::text(if s.settled { "final" } else { "provisional" }),
                    Cell::text(
                        s.pending
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    Cell::num(s.remark.as_ref().map(exact_text), 2),
                    Cell::num(s.remark_possible.as_ref().map(exact_text), 2),
                    Cell::text(s.comparison()),
                    Cell::num(Some(exact_text(&s.answers_only)), 2),
                    Cell::text(s.grade_ours.clone()),
                    Cell::text(s.grade_remark.clone()),
                ])
            })
            .collect();
        csv(&banner_sheet(
            "Students",
            "per student",
            &[
                "booklet",
                "our_student_id",
                "remark_student_id",
                "paired_by",
                "version",
                "our_total_from",
                "our_total",
                "max",
                "our_status",
                "pending_questions",
                "remark_total",
                "remark_total_possible",
                "comparison",
                "answers_only_total",
                "our_grade",
                "remark_grade",
            ],
            rows,
        ))
    }

    /// Identity differences CSV (key sheets graded as students included).
    pub fn identity_csv(&self) -> Result<Vec<u8>, AppError> {
        let mut rows: Vec<Row> = self
            .identity
            .iter()
            .map(|d| {
                Row::new(vec![
                    Cell::text(d.kind.describe()),
                    Cell::text(d.booklet.clone()),
                    Cell::Id(d.our_id.clone()),
                    Cell::text(d.our_flags.join(" ")),
                    Cell::text(d.remark_row.clone()),
                    Cell::Id(d.remark_id.clone()),
                    Cell::Id(d.handwritten.join(" ")),
                    Cell::text(d.detail.clone()),
                ])
            })
            .collect();
        for k in &self.key_sheets {
            for (row, id, _) in &k.rows {
                rows.push(Row::new(vec![
                    Cell::text("Remark key sheet graded as a student"),
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    Cell::text(format!("{} row {row}", k.workbook)),
                    Cell::Id(id.clone()),
                    Cell::Empty,
                    Cell::text("ID of one repeated digit"),
                ]));
            }
            for (row, id) in &k.possible {
                rows.push(Row::new(vec![
                    Cell::text("possible key sheet or perfect score"),
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    Cell::text(format!("{} row {row}", k.workbook)),
                    Cell::Id(id.clone()),
                    Cell::Empty,
                    Cell::text(
                        "answers identical to the key with full marks under a student-like ID, \
                         no booklet here: check whether it is the instructor's key sheet",
                    ),
                ]));
            }
        }
        csv(&banner_sheet(
            "Identity",
            "identity differences",
            &[
                "kind",
                "booklet",
                "our_student_id",
                "our_findings",
                "remark_row",
                "remark_student_id",
                "handwritten",
                "detail",
            ],
            rows,
        ))
    }

    /// The aggregate counts as JSON (with the banner).
    pub fn summary_json(&self) -> Value {
        json!({
            "banner": RESTRICTED_BANNER,
            "exam": self.exam,
            "versions": self.import.versions.iter().map(|v| json!({
                "form": v.form,
                "points_source": v.points_source.describe(),
                "total_possible": exact_text(&v.total_possible),
                "totals_reproduced": v.totals_reproduced,
                "rows_checked": v.rows_checked,
            })).collect::<Vec<_>>(),
            "workbooks": self.workbooks.iter().map(|w| json!({
                "file": w.file, "form": w.form, "rows": w.rows, "key_sheets": w.key_sheets,
                "paired": w.paired, "in_scope": w.in_scope,
            })).collect::<Vec<_>>(),
            "counts": self.counts,
            "totals_from": "the exam's score run (its points, voids and policy); answers_only_equal \
                is the readings check with Remark's key and points",
            "points_differences": self.points_differences.iter().map(|(f, q, o, r)| json!({
                "form": f, "question": q, "exam": o, "remark": r,
            })).collect::<Vec<_>>(),
            "maximum_differences": self.maximum_differences.iter().map(|(f, o, r)| json!({
                "form": f, "exam_maximum": o, "remark_total_possible": r,
            })).collect::<Vec<_>>(),
            "release_hold": self.hold_text(),
            "licence_review": self.licence,
            "key_sheet_effects": self.key_sheets.iter().map(|k| json!({
                "workbook": k.workbook,
                "rows": k.rows.len(),
                "possible": k.possible.len(),
                "without_possible": k.without_possible,
                "reported": k.reported,
                "with": k.with,
                "without": k.without,
            })).collect::<Vec<_>>(),
        })
    }

    /// The report as a self-contained HTML page (crops embedded as PNG).
    pub fn html(&self) -> String {
        let c = &self.counts;
        let mut h = String::new();
        h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
        h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
        h.push_str(&format!(
            "<title>Parallel run · {}</title>",
            esc(&self.exam)
        ));
        h.push_str(
            "<style>:root{--bg:#fff;--fg:#1d2433;--mut:#5b6475;--line:#d9dde5;--hi:#fff4d6;\
             --hib:#b45309;--ok:#1a7f37;--bad:#b42318;--card:#f6f7f9}\
             @media (prefers-color-scheme:dark){:root{--bg:#14171c;--fg:#e6e9ef;--mut:#9aa3b2;\
             --line:#2c323c;--hi:#3a2f12;--hib:#f0b44c;--ok:#4cc270;--bad:#ff6b5b;\
             --card:#1b1f26}}body{background:var(--bg);color:var(--fg);\
             font:15px/1.45 system-ui,sans-serif;margin:0;padding:16px;max-width:1200px}\
             h1{font-size:22px;margin:0 0 4px}h2{font-size:18px;margin:28px 0 8px}\
             .mut{color:var(--mut)}.banner{border:2px solid var(--bad);background:var(--hi);\
             padding:10px 12px;border-radius:6px;margin:12px 0;font-weight:600}\
             table{border-collapse:collapse;width:100%;font-size:14px}\
             th,td{border-bottom:1px solid var(--line);padding:4px 8px;text-align:left;\
             vertical-align:middle}td.n{font-variant-numeric:tabular-nums;text-align:right}\
             tr.hi td{background:var(--hi)}.crop{max-width:100%;height:auto;background:#fff;\
             border:1px solid var(--line)}circle{fill:none;stroke-width:2}\
             circle.f{stroke:#1a7f37}circle.a{stroke:#d97706;stroke-dasharray:4 3}\
             circle.u{stroke:#b42318}circle.b{stroke:none}.wrap{overflow-x:auto}\
             code{background:var(--card);padding:1px 4px;border-radius:4px}</style></head><body>",
        );
        h.push_str(&format!(
            "<div class=\"banner\" role=\"alert\">{}</div>",
            esc(RESTRICTED_BANNER)
        ));
        h.push_str(&format!(
            "<h1>Parallel run with Remark Office OMR</h1><p class=\"mut\">{} · {} version(s) · \
             {} questions · {} booklets · {} Remark rows in {} workbook(s)</p>",
            esc(&self.exam),
            self.forms.len(),
            self.questions,
            c.booklets,
            c.remark_rows,
            self.workbooks.len()
        ));
        h.push_str(&format!(
            "<p class=\"mut\">{} Agreement between the two systems is not a label: errors both \
             make are invisible here.</p><p class=\"mut\">{}</p>",
            esc(&self.hold_text()),
            esc(&self.licence)
        ));
        // Points and key.
        h.push_str(
            "<h2>Key and points</h2><table><tr><th>Version</th><th>Remark's points</th>\
                    <th>Remark's total possible</th><th>This exam's maximum</th>\
                    <th>Remark totals reproduced</th></tr>",
        );
        for v in &self.import.versions {
            let ours = self
                .maximum_differences
                .iter()
                .find(|m| m.0 == v.form)
                .map(|m| m.1.clone());
            h.push_str(&format!(
                "<tr{}><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                 <td class=\"n\">{} of {}</td></tr>",
                if ours.is_some() { " class=\"hi\"" } else { "" },
                esc(&v.form),
                esc(v.points_source.describe()),
                exact_text(&v.total_possible),
                ours.unwrap_or_else(|| exact_text(&v.total_possible)),
                v.totals_reproduced,
                v.rows_checked
            ));
        }
        h.push_str("</table>");
        if !self.maximum_differences.is_empty() {
            h.push_str(&format!(
                "<p class=\"banner\">This exam's maximum differs from Remark's total possible \
                 ({}): the exam is graded with other points than Remark's, so no total below can \
                 equal Remark's. Check the points before comparing totals (the key's points \
                 column, or grade with the points imported from Remark).</p>",
                esc(&self
                    .maximum_differences
                    .iter()
                    .map(|(f, o, r)| format!("version {f}: {o} here, {r} in Remark"))
                    .collect::<Vec<_>>()
                    .join("; "))
            ));
        }
        if self.points_differences.is_empty() {
            h.push_str("<p>This exam's points equal Remark's for every question.</p>");
        } else {
            h.push_str(&format!(
                "<p><b>{} question(s) carry other points in this exam than in Remark</b> (the \
                 totals below are this exam's):</p><table><tr><th>Version</th><th>Question</th>\
                 <th>This exam</th><th>Remark</th></tr>",
                self.points_differences.len()
            ));
            for (f, q, o, r) in &self.points_differences {
                h.push_str(&format!(
                    "<tr class=\"hi\"><td>{}</td><td class=\"n\">{q}</td><td class=\"n\">{}</td>\
                     <td class=\"n\">{}</td></tr>",
                    esc(f),
                    esc(o),
                    esc(r)
                ));
            }
            h.push_str("</table>");
        }
        if self.key_differences.is_empty() {
            h.push_str("<p>Remark's answer key equals this exam's key for every version.</p>");
        } else {
            h.push_str(&format!(
                "<p><b>{} key entr(ies) differ</b> (the totals below use this exam's key):</p>\
                 <table><tr><th>Version</th><th>Question</th><th>Remark</th><th>This exam</th>\
                 </tr>",
                self.key_differences.len()
            ));
            for (f, q, r, o) in &self.key_differences {
                h.push_str(&format!(
                    "<tr class=\"hi\"><td>{}</td><td class=\"n\">{q}</td><td>{}</td><td>{}</td>\
                     </tr>",
                    esc(f),
                    esc(r),
                    esc(o)
                ));
            }
            h.push_str("</table>");
        }
        // Workbooks.
        h.push_str(
            "<h2>Workbooks</h2><table><tr><th>Workbook</th><th>Version</th>\
                    <th>Rows</th><th>Key-sheet rows</th><th>Paired</th><th>Compared</th></tr>",
        );
        for w in &self.workbooks {
            h.push_str(&format!(
                "<tr{}><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                 <td class=\"n\">{}</td><td>{}</td></tr>",
                if w.in_scope { "" } else { " class=\"hi\"" },
                esc(&w.file),
                esc(&w.form),
                w.rows,
                w.key_sheets,
                w.paired,
                if w.in_scope {
                    "yes"
                } else {
                    "no: none of its rows is a booklet of this exam (another section's sheets)"
                }
            ));
        }
        h.push_str("</table>");
        // Answers.
        h.push_str(&format!(
            "<h2>Answers</h2><table><tr><th>Booklets paired</th><td class=\"n\">{} by student \
             ID, {} by student ID completing our incomplete read, {} by answers</td></tr><tr><th>Questions compared</th><td class=\"n\">{}</td>\
             </tr><tr><th>Equal, settled</th><td class=\"n\">{}</td></tr><tr><th>Equal, ours \
             pending review</th><td class=\"n\">{}</td></tr><tr><th>Different, settled</th><td \
             class=\"n\">{}</td></tr><tr><th>Different, ours pending review</th><td \
             class=\"n\">{}</td></tr></table>",
            c.paired_by_id,
            c.paired_by_partial_id,
            c.paired_by_answers,
            c.questions,
            c.agree,
            c.agree_pending,
            c.differ,
            c.differ_pending
        ));
        for (title, differ) in [
            ("Differences to adjudicate", true),
            (
                "Questions in review here (Remark read them as our proposal; decide them in \
                 the review queue)",
                false,
            ),
        ] {
            let rows: Vec<&Disagreement> = self
                .disagreements
                .iter()
                .filter(|d| (d.agreement != Agreement::AgreePending) == differ)
                .collect();
            if rows.is_empty() {
                continue;
            }
            h.push_str(&format!(
                "<h3>{}</h3><div class=\"wrap\"><table><tr><th>Booklet</th><th>Student</th>\
                 <th>Q</th><th>Remark</th><th>Ours</th><th>Our status</th><th>Our findings</th>\
                 <th>Bubble row as scanned</th></tr>",
                esc(title)
            ));
            for d in rows {
                h.push_str(&format!(
                    "<tr class=\"hi\"><td>{}</td><td>{}</td><td class=\"n\">{}</td><td><b>{}</b>\
                     </td><td><b>{}</b></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&d.page),
                    esc(&d.student),
                    d.question,
                    esc(&d.remark),
                    esc(&d.ours),
                    esc(&d.status),
                    esc(&d.findings),
                    d.crop.as_ref().map(crop_svg).unwrap_or_default()
                ));
            }
            h.push_str("</table></div>");
        }
        // Totals.
        h.push_str(&format!(
            "<h2>Totals</h2><p>This exam's totals are its score run: the figures it releases, \
             with its own points, voids and policy. Paired booklets graded and fully settled: {} \
             (total and maximum equal to Remark's: {}). Provisional: {} (equal to Remark's once \
             each pending question is resolved as Remark read it: {}); a pending question is \
             never credited.{} Answers only (a check of the readings, not of the grades: \
             Remark's key and points applied to our answers): {} of the paired booklets equal \
             Remark's total.{}</p>",
            c.totals_settled,
            c.totals_settled_equal,
            c.totals_provisional,
            c.totals_provisional_equal_if_remark,
            if c.totals_not_graded > 0 {
                format!(
                    " Not graded by this exam (no key or no version), compared on the answers \
                     only: {}.",
                    c.totals_not_graded
                )
            } else {
                String::new()
            },
            c.answers_only_equal,
            if c.objective_subtotals > 0 {
                format!(
                    " Learning objectives (CLO results): {} of {} subtotals of settled booklets \
                     equal Remark's (the exam's points on each objective's questions).",
                    c.objective_subtotals_equal, c.objective_subtotals
                )
            } else {
                String::new()
            }
        ));
        h.push_str(
            "<div class=\"wrap\"><table><tr><th>Booklet</th><th>Our ID</th>\
                    <th>Remark ID</th><th>Paired by</th><th>Version</th><th>This exam</th>\
                    <th>Remark</th><th>Answers only</th><th>Status</th><th>Grade (ours)</th>\
                    <th>Grade (Remark)</th></tr>",
        );
        for s in &self.students {
            let differs = !s.equals_remark();
            h.push_str(&format!(
                "<tr{}><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
                 <td class=\"n\">{} / {}{}</td><td class=\"n\">{}{}</td><td class=\"n\">{}</td>\
                 <td>{}</td><td>{}</td><td>{}</td></tr>",
                if differs { " class=\"hi\"" } else { "" },
                esc(&s.label),
                esc(&s.our_id),
                esc(&s.remark_id),
                esc(&s.how),
                esc(&s.form),
                exact_text(&s.ours),
                exact_text(&s.max),
                if s.graded { "" } else { " (not graded)" },
                s.remark.as_ref().map(exact_text).unwrap_or_default(),
                s.remark_possible
                    .as_ref()
                    .filter(|_| s.remark.is_some())
                    .map(|x| format!(" / {}", exact_text(x)))
                    .unwrap_or_default(),
                exact_text(&s.answers_only),
                if s.settled {
                    "final".to_string()
                } else {
                    format!(
                        "provisional (pending: {})",
                        s.pending
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
                esc(&s.grade_ours),
                esc(&s.grade_remark)
            ));
        }
        h.push_str("</table></div>");
        // Identity.
        h.push_str("<h2>Identity differences</h2>");
        if self.identity.is_empty() {
            h.push_str("<p>None.</p>");
        } else {
            h.push_str(
                "<div class=\"wrap\"><table><tr><th>Kind</th><th>Booklet</th>\
                        <th>Our ID</th><th>Our findings</th><th>Remark row</th><th>Remark ID</th>\
                        <th>Detail</th></tr>",
            );
            for d in &self.identity {
                h.push_str(&format!(
                    "<tr class=\"hi\"><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
                     <td>{}</td><td>{}</td></tr>",
                    esc(d.kind.describe()),
                    esc(&d.booklet),
                    esc(&d.our_id),
                    esc(&d.our_flags.join(", ")),
                    esc(&d.remark_row),
                    esc(&d.remark_id),
                    esc(&d.detail)
                ));
            }
            h.push_str("</table></div>");
        }
        // Key sheets.
        h.push_str("<h2>Key sheets graded as students in Remark</h2>");
        if self.key_sheets.is_empty() {
            h.push_str("<p>None.</p>");
        } else {
            h.push_str(
                "<p>Remark graded these rows as students; they enter its class statistics. \
                 Recomputed as Remark computes them (SD ÷ N, KR-20 on the number right, α on \
                 the weighted item scores), with and without the key-sheet rows:</p>\
                 <div class=\"wrap\"><table><tr><th>Workbook</th><th>Rows</th><th></th>\
                 <th>N</th><th>Mean</th><th>SD</th><th>Top</th><th>Top %</th><th>KR-20</th>\
                 <th>α</th></tr>",
            );
            for k in &self.key_sheets {
                let rows = k
                    .rows
                    .iter()
                    .map(|(r, id, _)| format!("row {r} (ID {id})"))
                    .chain(k.possible.iter().map(|(r, id)| {
                        format!("row {r} (ID {id}, possible key sheet or perfect score)")
                    }))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(rp) = &k.reported {
                    h.push_str(&format!(
                        "<tr><td>{}</td><td>{}</td><td>Remark reported</td><td></td>\
                         <td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                         <td></td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
                        esc(&k.workbook),
                        esc(&rows),
                        opt(rp.mean, 3),
                        opt(rp.sd, 3),
                        opt(rp.max, 2),
                        opt(rp.kr20, 3),
                        opt(rp.alpha, 3)
                    ));
                }
                let mut lines = vec![
                    ("recomputed, every row", &k.with),
                    ("recomputed, without the key sheets", &k.without),
                ];
                if let Some(wp) = &k.without_possible {
                    lines.push((
                        "recomputed, without the possible key sheets too (rows with every \
                         answer right and no booklet: a key sheet or a perfect score)",
                        wp,
                    ));
                }
                for (what, st) in lines {
                    h.push_str(&format!(
                        "<tr{}><td>{}</td><td></td><td>{what}</td><td class=\"n\">{}</td>\
                         <td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                         <td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>\
                         </tr>",
                        if what.contains("without") {
                            " class=\"hi\""
                        } else {
                            ""
                        },
                        if k.reported.is_none() && what.contains("every") {
                            esc(&format!("{} ({rows})", k.workbook))
                        } else {
                            String::new()
                        },
                        st.n,
                        opt(st.mean, 3),
                        opt(st.sd, 3),
                        opt(st.max, 2),
                        opt(st.max_percent, 1),
                        opt(st.kr20, 3),
                        opt(st.alpha, 3)
                    ));
                }
            }
            h.push_str("</table></div>");
        }
        if !self.import.notes.is_empty() {
            h.push_str("<h2>Notes</h2><ul>");
            for n in &self.import.notes {
                h.push_str(&format!("<li>{}</li>", esc(n)));
            }
            h.push_str("</ul>");
        }
        h.push_str(&format!(
            "<p class=\"banner\">{}</p></body></html>",
            esc(RESTRICTED_BANNER)
        ));
        h
    }
}

fn csv(s: &Sheet) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    omr_report::write_csv(&mut out, s).map_err(|e| AppError::Internal(format!("report: {e}")))?;
    Ok(out)
}

/// Reads handwritten student numbers (`file,page,student_id`, one row per scan page; several
/// readings of one page in several rows): the comparison names a Remark ID equal to one of
/// them (an operator's correction in Remark).
pub fn read_handwritten_csv(bytes: &[u8]) -> Result<Vec<HandwrittenId>, AppError> {
    let text = crate::exam::decode_text(bytes);
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(crate::exam::sniff_delimiter(&text))
        .from_reader(text.as_bytes());
    let fix = "write a header `file,page,student_id` and one row per page";
    let h: Vec<String> = r
        .headers()
        .map_err(|e| AppError::Input(format!("handwritten IDs: {e}; {fix}")))?
        .iter()
        .map(|x| x.trim().to_ascii_lowercase())
        .collect();
    let col = |n: &[&str]| h.iter().position(|x| n.contains(&x.as_str()));
    let (Some(cf), Some(cp), Some(ci)) = (
        col(&["file", "source"]),
        col(&["page"]),
        col(&["student_id", "student id", "id", "handwritten"]),
    ) else {
        return Err(AppError::Input(format!(
            "handwritten IDs: columns {h:?}; {fix}"
        )));
    };
    let mut out: Vec<HandwrittenId> = Vec::new();
    for (k, rec) in r.records().enumerate() {
        let rec = rec.map_err(|e| AppError::Input(format!("handwritten IDs: {e}; {fix}")))?;
        let get = |c: usize| rec.get(c).map(str::trim).unwrap_or("").to_string();
        let page: u32 = get(cp).parse().map_err(|_| {
            AppError::Input(format!(
                "handwritten IDs line {}: page `{}` is not a number; {fix}",
                k + 2,
                get(cp)
            ))
        })?;
        let (file, id) = (get(cf), get(ci));
        if file.is_empty() || id.is_empty() {
            continue;
        }
        match out.iter_mut().find(|x| x.file == file && x.page == page) {
            Some(x) => x.ids.push(id),
            None => out.push(HandwrittenId {
                file,
                page,
                ids: vec![id],
            }),
        }
    }
    Ok(out)
}

/// File names of the report.
pub const REPORT_FILES: &[&str] = &[
    "parallel_run.html",
    "parallel_run_students.csv",
    "parallel_run_questions.csv",
    "parallel_run_identity.csv",
    "parallel_run_summary.json",
];

impl ExamProject {
    /// Compares this exam with Remark's graded workbooks of the same sheets ([`compare`]).
    pub fn compare_remark(
        &self,
        workbooks: &[RemarkWorkbook],
        opts: &CompareOptions,
    ) -> Result<ParallelRun, AppError> {
        compare(self, workbooks, opts)
    }

    /// Writes the parallel-run report into `dir` (where the user asks: refused in a sync or
    /// network folder, SYS-015; created private; files owner-only) with a signed manifest
    /// (label `restricted`, purpose `parallel-run`), and records `ExportCreated`.
    pub fn export_parallel_run(
        &mut self,
        run: &ParallelRun,
        dir: &Path,
    ) -> Result<crate::export::ExportSummary, AppError> {
        self.check_audited()?;
        let files: Vec<(String, Vec<u8>)> = vec![
            (REPORT_FILES[0].into(), run.html().into_bytes()),
            (REPORT_FILES[1].into(), run.students_csv()?),
            (REPORT_FILES[2].into(), run.questions_csv()?),
            (REPORT_FILES[3].into(), run.identity_csv()?),
            (
                REPORT_FILES[4].into(),
                serde_json::to_vec_pretty(&run.summary_json())
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            ),
        ];
        let summary = crate::export::write_signed(
            self,
            dir,
            &format!("{} parallel run with Remark", self.session.definition.title),
            "parallel-run",
            &files,
        )?;
        self.record_export(
            "parallel-run",
            &format!("folder:{}", dir.display()),
            &summary.manifest_sha384,
        )?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_with_lost_leading_zeros_only_from_number_cells() {
        assert!(same_id("221000001", true, "0221000001"));
        assert!(!same_id("221000001", false, "0221000001"));
        assert!(same_id("2210000001", false, "2210000001"));
        assert!(!same_id("", true, "2210000001"));
    }
}
