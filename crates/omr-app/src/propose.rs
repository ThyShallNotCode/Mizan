//! The New-exam proposal (G-3): every value of a new exam's definition proposed from evidence,
//! each with its source, for the desktop's New-exam card and `omr survey`. The shells show the
//! proposal and forward the instructor's choices; they decide nothing (§4.7).
//!
//! **The contract.**
//! 1. Every value shown is the value graded: [`Proposal::inputs`] is built from the same fields
//!    and checked by [`define::define`], the single validation path.
//! 2. A value comes from evidence, in this order: what the instructor typed (never replaced,
//!    only checked), the Remark import, the key file, the key sheets, the students' sheets
//!    ([`Survey`]), the sheet or its spec file, the exams folder, and last a default.
//! 3. A contradiction at exam level stops with a question ([`Level::Ask`]) whose choices are
//!    one click each; [`Proposal::inputs`] is `None` while one is open.
//! 4. An individual anomaly (one sheet bubbling a stray version) is a note: grading holds that
//!    sheet for review (§10.3 backstops), never scores it under another version's key.
//! 5. A provisional (inferred) key is never a default: only [`Answer::InferKeyProvisionally`]
//!    chooses it.
//!
//! [`propose`] is pure apart from reading the small key, roster, CLO and spec files and listing
//! the scan files (never reading them). Its thresholds are untuned and live in one place,
//! [`Thresholds`].

use crate::AppError;
use crate::define::{self, Inputs, PrintedSheet, Sheet};
use crate::exam::{self, ExamDefinition, KeyShape, KeyVersions, SetupField, TemplateChoice};
use crate::export::mask_numbers;
use crate::remark::RemarkImport;
use crate::survey::{self, Survey, share_at_least};
use omr_model::CancelMode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The thresholds of the proposal's checks. None is tuned on real stacks yet: each is named
/// here, so a tuning run changes one place (shares are in ‰ of the surveyed sheets, n).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// A version letter is **substantial** when at least this many sheets bubble it (every
    /// sheet, in a smaller stack: a make-up stack of one or two sheets)…
    pub substantial_min: u32,
    /// …and at least this share of the sheets (5 %); a letter on fewer sheets is a **stray**,
    /// held sheet by sheet, never a reason to declare the version.
    pub substantial_permille: u32,
    /// After [`Answer::InferKeyProvisionally`], a version is declared when at least this many
    /// sheets bubble it (enough answers to infer a key from)…
    pub infer_min: u32,
    /// …and at least this share of the sheets.
    pub infer_permille: u32,
    /// N50, the students' extent, is the last row marked on at least this share (50 %)…
    pub n50_permille: u32,
    /// …and the extent is **high** (N50 can serve as the question count) when row N50 is
    /// marked on at least this share (70 %)…
    pub high_at_n50_permille: u32,
    /// …no row beyond it on more than this share (5 %)…
    pub high_tail_permille: u32,
    /// …and every row up to it on at least this share (30 %).
    pub high_body_permille: u32,
    /// Rows beyond the question count answered on at least this share (10 %)…
    pub beyond_permille: u32,
    /// …and by at least this many sheets ask whether the count is too small.
    pub beyond_min: u32,
    /// A count more than this many rows above N50 (with a high extent) gives a note.
    pub speeded_rows: u16,
    /// Two exams in one stack: at least this many sheets…
    pub short_min: u32,
    /// …and this share (15 %) end by half of N50…
    pub short_permille: u32,
    /// …or two exam-type bubbles each on at least this share (20 %).
    pub exam_type_permille: u32,
    /// The exam-type bubble names the exam when its most common value is on this share (60 %).
    pub title_permille: u32,
    /// Fewer complete student numbers than this share (90 %) fitting the ID scheme give a
    /// note…
    pub id_fit_permille: u32,
    /// …and a typed scheme that fits fewer than this share (50 %) while the sheet's own fits
    /// gives a question.
    pub id_misfit_permille: u32,
    /// The surveyed version counts may differ from the Remark export's by this many sheets.
    pub remark_count_slack: u32,
    /// The workspace's last exam gives its term when graded within this many days.
    pub term_days: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            substantial_min: 3,
            substantial_permille: 50,
            infer_min: 10,
            infer_permille: 50,
            n50_permille: 500,
            high_at_n50_permille: 700,
            high_tail_permille: 50,
            high_body_permille: 300,
            beyond_permille: 100,
            beyond_min: 3,
            speeded_rows: 2,
            short_min: 5,
            short_permille: 150,
            exam_type_permille: 200,
            title_permille: 600,
            id_fit_permille: 900,
            id_misfit_permille: 500,
            remark_count_slack: 2,
            term_days: 120,
        }
    }
}

/// A field of the New-exam card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldId {
    /// The scan files and folders.
    Scans,
    /// The printed sheet (template or spec file).
    Sheet,
    /// The answer key.
    Key,
    /// Key-sheet files.
    KeySheets,
    /// The Remark import.
    Remark,
    /// The versions (forms).
    Versions,
    /// The number of questions.
    Questions,
    /// Options per question.
    Options,
    /// The marking convention.
    CancelMode,
    /// The student-ID scheme.
    IdScheme,
    /// The roster file.
    Roster,
    /// The CLO mapping file.
    Clo,
    /// The title.
    Title,
    /// The course code.
    Course,
    /// The term.
    Term,
    /// Second review of confident double marks.
    MultiReview,
    /// High-stakes mode.
    HighStakes,
    /// The output folder.
    Folder,
}

impl FieldId {
    /// Every field, in the card's order.
    pub const ALL: [FieldId; 18] = [
        FieldId::Scans,
        FieldId::Sheet,
        FieldId::Key,
        FieldId::KeySheets,
        FieldId::Remark,
        FieldId::Versions,
        FieldId::Questions,
        FieldId::Options,
        FieldId::CancelMode,
        FieldId::IdScheme,
        FieldId::Roster,
        FieldId::Clo,
        FieldId::Title,
        FieldId::Course,
        FieldId::Term,
        FieldId::MultiReview,
        FieldId::HighStakes,
        FieldId::Folder,
    ];

    /// Its name (`questions`, `cancel-mode`).
    pub fn name(self) -> &'static str {
        match self {
            FieldId::Scans => "scans",
            FieldId::Sheet => "sheet",
            FieldId::Key => "key",
            FieldId::KeySheets => "key-sheets",
            FieldId::Remark => "remark",
            FieldId::Versions => "versions",
            FieldId::Questions => "questions",
            FieldId::Options => "options",
            FieldId::CancelMode => "cancel-mode",
            FieldId::IdScheme => "id-scheme",
            FieldId::Roster => "roster",
            FieldId::Clo => "clo",
            FieldId::Title => "title",
            FieldId::Course => "course",
            FieldId::Term => "term",
            FieldId::MultiReview => "multi-review",
            FieldId::HighStakes => "high-stakes",
            FieldId::Folder => "folder",
        }
    }

    /// The field of a name ([`Self::name`]).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s.trim())
    }

    /// Its label on the card.
    pub fn label(self) -> &'static str {
        match self {
            FieldId::Scans => "Scans",
            FieldId::Sheet => "Sheet",
            FieldId::Key => "Answer key",
            FieldId::KeySheets => "Key sheets",
            FieldId::Remark => "Remark import",
            FieldId::Versions => "Versions",
            FieldId::Questions => "Questions",
            FieldId::Options => "Options",
            FieldId::CancelMode => "Cancel mode",
            FieldId::IdScheme => "Student ID",
            FieldId::Roster => "Roster",
            FieldId::Clo => "CLO mapping",
            FieldId::Title => "Title",
            FieldId::Course => "Course",
            FieldId::Term => "Term",
            FieldId::MultiReview => "Second review",
            FieldId::HighStakes => "High stakes",
            FieldId::Folder => "Folder",
        }
    }
}

/// Where a value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Typed (or chosen) on the form.
    Typed,
    /// The instructor's answer to a question of the proposal.
    Answer,
    /// The Remark import.
    RemarkImport,
    /// The answer-key file.
    KeyFile,
    /// The key sheets.
    KeySheets,
    /// The students' sheets (the survey).
    Sheets,
    /// The own sheet's spec file.
    SpecFile,
    /// The bundled template (the IAU sheet).
    Template,
    /// The scans' folder name.
    Folder,
    /// The exams folder (the last exam graded there).
    Workspace,
    /// A default.
    Default,
}

impl Source {
    /// Its text on the card.
    pub fn text(self) -> &'static str {
        match self {
            Source::Typed => "typed",
            Source::Answer => "your answer",
            Source::RemarkImport => "Remark import",
            Source::KeyFile => "key file",
            Source::KeySheets => "key sheets",
            Source::Sheets => "the sheets",
            Source::SpecFile => "spec file",
            Source::Template => "the sheet",
            Source::Folder => "folder name",
            Source::Workspace => "exams folder",
            Source::Default => "default",
        }
    }
}

/// How a field stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldState {
    /// Nothing to say.
    Ok,
    /// A note on it.
    Note,
    /// A question on it is open.
    Ask,
}

/// One field of the card: value — source (evidence).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldView {
    /// The field.
    pub id: FieldId,
    /// Its value, as graded and as typed (`""`: none yet), e.g. `erase_only`.
    pub value: String,
    /// The value in plain words, as the card writes it (`Erase to change` for `erase_only`,
    /// `IAU student number` for `iau`); the value itself where it is plain already.
    #[serde(default)]
    pub plain: String,
    /// Where the value came from.
    pub source: Source,
    /// The evidence, e.g. `key.csv names A, B · sheets A 8 · B 8 · blank 2`.
    pub detail: String,
    /// Its issues.
    pub state: FieldState,
    /// The printed sheet fixes it (its cancel mode): the card shows it, never takes it typed.
    #[serde(default)]
    pub fixed: bool,
}

/// The values a field takes from a closed set (Second review), each with its plain words: the
/// card offers them as choices, never as text to type. Empty for any other field.
pub fn value_choices(f: FieldId) -> &'static [(&'static str, &'static str)] {
    match f {
        FieldId::MultiReview => MULTI_REVIEW,
        _ => &[],
    }
}

/// Second review's values (`define`'s multi-review policy) and their plain words.
const MULTI_REVIEW: &[(&str, &str)] = &[
    ("never", "No second review"),
    ("always", "Always"),
    ("high_stakes", "On high-stakes exams"),
];

/// A question or a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// A contradiction at exam level: nothing is graded until it is answered.
    Ask,
    /// Shown, never blocking.
    Note,
}

/// An issue of the proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// The field it is about.
    pub field: FieldId,
    /// Question or note.
    pub level: Level,
    /// What the evidence shows and what is at stake (counts and file names only).
    pub text: String,
    /// One-click answers (questions only).
    pub choices: Vec<Choice>,
}

/// One answer to a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Its button text.
    pub label: String,
    /// What it does.
    pub action: ChoiceAction,
}

/// What a choice does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChoiceAction {
    /// Add this answer to [`Given::answers`] and propose again.
    Answer(Answer),
    /// Pick a file or folder for this field (the key file, key sheets, scans, the sheet's spec,
    /// a Remark export …), then propose again.
    Pick(FieldId),
    /// Clear this field (e.g. the key file when a Remark import gives the key).
    Clear(FieldId),
    /// Type a value in this field.
    Type(FieldId),
    /// Survey the scans again.
    Survey,
}

/// The instructor's answer to a question of the proposal ([`Given::answers`]). An answer that
/// names a value wins over the typed one: it was given knowing both. It holds only while the
/// field keeps the value it was given for: a field typed again drops the answers about it
/// ([`Given::drop_answers_for_retyped`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    /// Grade with a key inferred from the sheets: provisional, not releasable until the real
    /// key replaces it.
    InferKeyProvisionally,
    /// The one answer column of the key file is the key of every version the sheets bubble.
    SameKeyForEveryVersion,
    /// These versions are not part of the exam: their sheets are held, never scored.
    HoldVersions(Vec<String>),
    /// The key file's versions, in the file's order, are these letters (numbered versions,
    /// unlabelled keys, or a key version read as another).
    KeyVersions(Vec<String>),
    /// The key file's headerless rows hold answers alone: the letter each starts with is its
    /// answer to question 1, not its version (the rows are unlabelled keys).
    KeyRowsUnlabelled,
    /// The exam has this many questions.
    UseQuestions(u16),
    /// The exam has this many questions: key entries and students' rows beyond it are not
    /// part of it.
    KeepQuestions(u16),
    /// Grade although the key has no entry for some questions: nothing is scored until each
    /// is filled in or voided.
    GradeWithKeyGaps,
    /// Keep the typed value of this field although the evidence speaks against it.
    UseTypedValue(FieldId),
    /// Use this value for the field.
    UseValue(FieldId, String),
    /// The scans hold one exam.
    OneExam,
    /// Grade with this sheet although many pages do not register as it (they are held).
    KeepSheet,
}

impl Answer {
    /// The field whose value the answer gives or keeps (`None`: an answer about the files or
    /// the stack, which holds while they stay).
    pub fn field(&self) -> Option<FieldId> {
        match self {
            Answer::UseValue(f, _) | Answer::UseTypedValue(f) => Some(*f),
            Answer::UseQuestions(_) | Answer::KeepQuestions(_) => Some(FieldId::Questions),
            _ => None,
        }
    }

    /// The answer as one word for the command line (`omr survey --answer`): `provisional-key`,
    /// `same-key`, `hold:C,D`, `key-versions:A,B`, `unlabelled-rows`, `questions:40`,
    /// `keep-questions:30`, `key-gaps`, `keep:id-scheme`, `use:options=5`, `one-exam`,
    /// `keep-sheet`.
    pub fn token(&self) -> String {
        match self {
            Answer::InferKeyProvisionally => "provisional-key".into(),
            Answer::SameKeyForEveryVersion => "same-key".into(),
            Answer::HoldVersions(v) => format!("hold:{}", v.join(",")),
            Answer::KeyVersions(v) => format!("key-versions:{}", v.join(",")),
            Answer::KeyRowsUnlabelled => "unlabelled-rows".into(),
            Answer::UseQuestions(n) => format!("questions:{n}"),
            Answer::KeepQuestions(n) => format!("keep-questions:{n}"),
            Answer::GradeWithKeyGaps => "key-gaps".into(),
            Answer::UseTypedValue(f) => format!("keep:{}", f.name()),
            Answer::UseValue(f, v) => format!("use:{}={v}", f.name()),
            Answer::OneExam => "one-exam".into(),
            Answer::KeepSheet => "keep-sheet".into(),
        }
    }

    /// The answer of a [`Self::token`].
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (word, arg) = s.split_once(':').unwrap_or((s, ""));
        let letters = |a: &str| letters(a).filter(|v| !v.is_empty());
        let number = |a: &str| a.trim().parse::<u16>().ok().filter(|n| *n > 0);
        let field = |a: &str| FieldId::parse(a);
        let bad = || {
            format!(
                "--answer {s}: use provisional-key, same-key, hold:C, key-versions:A,B, \
                 unlabelled-rows, questions:N, keep-questions:N, key-gaps, keep:FIELD, \
                 use:FIELD=VALUE, one-exam or keep-sheet"
            )
        };
        Ok(match word {
            "provisional-key" => Answer::InferKeyProvisionally,
            "same-key" => Answer::SameKeyForEveryVersion,
            "hold" => Answer::HoldVersions(letters(arg).ok_or_else(bad)?),
            "key-versions" => Answer::KeyVersions(letters(arg).ok_or_else(bad)?),
            "unlabelled-rows" => Answer::KeyRowsUnlabelled,
            "questions" => Answer::UseQuestions(number(arg).ok_or_else(bad)?),
            "keep-questions" => Answer::KeepQuestions(number(arg).ok_or_else(bad)?),
            "key-gaps" => Answer::GradeWithKeyGaps,
            "keep" => Answer::UseTypedValue(field(arg).ok_or_else(bad)?),
            "use" => {
                let (f, v) = arg.split_once('=').ok_or_else(bad)?;
                Answer::UseValue(field(f).ok_or_else(bad)?, v.trim().to_string())
            }
            "one-exam" => Answer::OneExam,
            "keep-sheet" => Answer::KeepSheet,
            _ => return Err(bad()),
        })
    }
}

/// What the form holds as typed (`None`: left blank).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    /// Number of questions.
    pub questions: Option<String>,
    /// Options per question.
    pub options: Option<String>,
    /// Versions (`A,B`).
    pub forms: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// Course code.
    pub course: Option<String>,
    /// Term.
    pub term: Option<String>,
    /// `erase_only`, `cross_cancels` or `fill_cancels_check`.
    pub cancel_mode: Option<String>,
    /// `never`, `always` or `high_stakes`.
    pub multi_review: Option<String>,
    /// High-stakes mode.
    pub high_stakes: Option<bool>,
    /// `iau`, `pnu`, `ksu` or `digits:N`.
    pub id_scheme: Option<String>,
    /// The output folder (Advanced); a relative one is below the exams folder.
    pub folder: Option<PathBuf>,
}

impl Typed {
    /// The fields typed otherwise in `now` than here: given, changed or emptied (text compared
    /// trimmed).
    pub fn retyped(&self, now: &Typed) -> Vec<FieldId> {
        let t = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let text = [
            (FieldId::Questions, &self.questions, &now.questions),
            (FieldId::Options, &self.options, &now.options),
            (FieldId::Versions, &self.forms, &now.forms),
            (FieldId::Title, &self.title, &now.title),
            (FieldId::Course, &self.course, &now.course),
            (FieldId::Term, &self.term, &now.term),
            (FieldId::CancelMode, &self.cancel_mode, &now.cancel_mode),
            (FieldId::MultiReview, &self.multi_review, &now.multi_review),
            (FieldId::IdScheme, &self.id_scheme, &now.id_scheme),
        ];
        let mut v: Vec<FieldId> = text
            .into_iter()
            .filter(|(_, a, b)| t(a) != t(b))
            .map(|(f, _, _)| f)
            .collect();
        if self.high_stakes != now.high_stakes {
            v.push(FieldId::HighStakes);
        }
        if self.folder != now.folder {
            v.push(FieldId::Folder);
        }
        v
    }
}

/// What the instructor gave for a new exam.
#[derive(Clone, Debug, PartialEq)]
pub struct Given {
    /// The printed sheet.
    pub sheet: Sheet,
    /// Scan files and folders.
    pub scans: Vec<PathBuf>,
    /// The answer-key file.
    pub key: Option<PathBuf>,
    /// Files named as key sheets.
    pub key_sheets: Vec<PathBuf>,
    /// A Remark import read for this exam.
    pub remark: Option<RemarkImport>,
    /// The roster file.
    pub roster: Option<PathBuf>,
    /// The CLO mapping file.
    pub clo: Option<PathBuf>,
    /// Values typed on the form.
    pub typed: Typed,
    /// Answers to the proposal's questions so far.
    pub answers: Vec<Answer>,
}

impl Given {
    /// Scans on `sheet`, nothing else given yet.
    pub fn new(sheet: Sheet, scans: Vec<PathBuf>) -> Self {
        Self {
            sheet,
            scans,
            key: None,
            key_sheets: Vec::new(),
            remark: None,
            roster: None,
            clo: None,
            typed: Typed::default(),
            answers: Vec::new(),
        }
    }

    /// Drops the answers about a field typed otherwise now than in `before` ([`Typed::retyped`]):
    /// an answer was given for the value the field held then, and the new value is checked
    /// afresh.
    pub fn drop_answers_for_retyped(&mut self, before: &Typed) {
        let retyped = before.retyped(&self.typed);
        self.answers
            .retain(|a| a.field().is_none_or(|f| !retyped.contains(&f)));
    }

    /// The survey request for these scans (never cancelled: set its `cancel` to stop it).
    pub fn survey_request(&self, threads: usize) -> survey::SurveyRequest {
        survey::SurveyRequest {
            sheet: self.sheet.clone(),
            scans: self.scans.clone(),
            key_sheets: self.key_sheets.clone(),
            roster: self.roster.clone(),
            threads,
            cancel: Default::default(),
        }
    }
}

/// An exam already in the exams folder (for the term).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentExam {
    /// Its term.
    pub term: String,
    /// Its course code.
    pub course: String,
    /// When it was graded (RFC 3339 UTC).
    pub graded_utc: String,
}

/// Where the exam is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// The exams folder: a new exam gets a new folder below it ([`define::new_exam_dir`]).
    pub workspace: PathBuf,
    /// Now (RFC 3339 UTC).
    pub now_utc: String,
    /// The exams already in the exams folder.
    pub recent: Vec<RecentExam>,
    /// The checks' thresholds.
    pub thresholds: Thresholds,
}

impl Context {
    /// The exams folder `workspace`, now, no earlier exams, default thresholds.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            now_utc: crate::pipeline::now_utc(),
            recent: Vec::new(),
            thresholds: Thresholds::default(),
        }
    }
}

/// The proposal for a new exam ([`propose`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// Every field, in the card's order.
    pub fields: Vec<FieldView>,
    /// Questions and notes.
    pub issues: Vec<Issue>,
    /// The inputs to grade with ([`define::define`] accepts them), only when no question is
    /// open.
    pub inputs: Option<Inputs>,
    /// The files to grade as key sheets ([`crate::pipeline::grade_with_key_sheets`]).
    pub key_sheets: Vec<PathBuf>,
    /// The exam's new folder (never one that holds a project).
    pub folder: PathBuf,
}

impl Proposal {
    /// The field `id`.
    pub fn field(&self, id: FieldId) -> Option<&FieldView> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// The open questions.
    pub fn asks(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Ask)
    }

    /// The notes.
    pub fn notes(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Note)
    }

    /// Why the exam cannot be graded yet (shown beside the Grade button), `None` when it can.
    pub fn blocker(&self) -> Option<String> {
        let asks: Vec<&Issue> = self.asks().collect();
        match asks.as_slice() {
            [] => None,
            [one] => Some(format!("{}: {}", one.field.label(), one.text)),
            [first, rest @ ..] => Some(format!(
                "{} questions to answer first; {}: {}",
                rest.len() + 1,
                first.field.label(),
                first.text
            )),
        }
    }
}

/// The letters of a typed list (`A, b C`), upper case, in order, without repeats.
fn letters(s: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for part in s
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let up = part.to_uppercase();
        if !out.contains(&up) {
            out.push(up);
        }
    }
    Some(out)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn plural(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

fn join(v: &[String]) -> String {
    v.join(", ")
}

/// The course code in a folder name (`CS211 Midterm` → `CS 211`): two to five letters, an
/// optional space or hyphen, three digits.
fn course_in(name: &str) -> Option<(String, std::ops::Range<usize>)> {
    let c: Vec<(usize, char)> = name.char_indices().collect();
    for s in 0..c.len() {
        if !c[s].1.is_ascii_alphabetic() || (s > 0 && c[s - 1].1.is_ascii_alphabetic()) {
            continue;
        }
        let mut e = s;
        while e < c.len() && c[e].1.is_ascii_alphabetic() {
            e += 1;
        }
        if !(2..=5).contains(&(e - s)) {
            continue;
        }
        let mut d = e;
        if d < c.len() && c[d].1 == ' ' {
            d += 1;
        }
        if d < c.len() && c[d].1 == '-' {
            d += 1;
        }
        let digits = c[d..].iter().take_while(|x| x.1.is_ascii_digit()).count();
        if digits == 3 {
            let letters: String = c[s..e].iter().map(|x| x.1.to_ascii_uppercase()).collect();
            let number: String = c[d..d + 3].iter().map(|x| x.1).collect();
            let end = c.get(d + 3).map_or(name.len(), |x| x.0);
            return Some((format!("{letters} {number}"), c[s].0..end));
        }
    }
    None
}

/// Days since 1970-01-01 of an RFC 3339 time's date.
fn days(utc: &str) -> Option<i64> {
    let y: i64 = utc.get(0..4)?.parse().ok()?;
    let m: i64 = utc.get(5..7)?.parse().ok()?;
    let d: i64 = utc.get(8..10)?.parse().ok()?;
    // Days from the civil date (H. Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The folder the scans are in (one folder given, or the first file's folder).
fn scans_folder(scans: &[PathBuf]) -> Option<String> {
    let first = scans.iter().min()?;
    let dir = if first.is_dir() {
        first.as_path()
    } else {
        first.parent()?
    };
    dir.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Folder names that say nothing about the exam (never a title).
const GENERIC_FOLDERS: &[&str] = &[
    "scans",
    "scan",
    "images",
    "pdf",
    "pdfs",
    "input",
    "inputs",
    "downloads",
    "desktop",
];

/// The scheme name of an ID scheme choice (`iau`, `digits:9`).
fn scheme_name(c: &exam::IdSchemeChoice) -> String {
    match c {
        exam::IdSchemeChoice::Iau => "iau".into(),
        exam::IdSchemeChoice::Pnu => "pnu".into(),
        exam::IdSchemeChoice::Ksu => "ksu".into(),
        exam::IdSchemeChoice::Digits(n) => format!("digits:{n}"),
    }
}

/// A scheme's plain words (`IAU student number`, `9-digit number`); `None` for a typed text
/// that names no scheme.
fn scheme_plain(name: &str) -> Option<String> {
    Some(match name {
        "iau" => "IAU student number".into(),
        "pnu" => "PNU student number".into(),
        "ksu" => "KSU student number".into(),
        s => format!(
            "{}-digit number",
            s.strip_prefix("digits:")?.parse::<usize>().ok()?
        ),
    })
}

fn cancel_name(m: CancelMode) -> &'static str {
    match m {
        CancelMode::EraseOnly => "erase_only",
        CancelMode::CrossCancels => "cross_cancels",
        CancelMode::FillCancelsCheck => "fill_cancels_check",
    }
}

/// A marking convention in plain words, and what it means for a mark.
fn cancel_plain(m: CancelMode) -> (&'static str, &'static str) {
    match m {
        CancelMode::EraseOnly => (
            "Erase to change",
            "students erase to change an answer; a crossed-out mark goes to review",
        ),
        CancelMode::CrossCancels => ("Cross out to cancel", "a crossed-out fill does not count"),
        CancelMode::FillCancelsCheck => (
            "Tick to answer, fill to cancel",
            "a tick chooses an option and a filled bubble cancels it",
        ),
    }
}

/// The plain words of a typed cancel mode, when it names one.
fn cancel_plain_of(name: &str) -> Option<&'static str> {
    [
        CancelMode::EraseOnly,
        CancelMode::CrossCancels,
        CancelMode::FillCancelsCheck,
    ]
    .into_iter()
    .find(|m| cancel_name(*m) == name)
    .map(|m| cancel_plain(m).0)
}

/// The card while it is built.
#[derive(Default)]
struct Card {
    fields: BTreeMap<FieldId, (String, Source, String)>,
    /// A value's plain words, where it is a code ([`FieldView::plain`]).
    plain: BTreeMap<FieldId, String>,
    /// The fields the printed sheet fixes ([`FieldView::fixed`]).
    fixed: BTreeSet<FieldId>,
    issues: Vec<Issue>,
}

impl Card {
    fn set(&mut self, id: FieldId, value: impl Into<String>, source: Source, detail: String) {
        self.fields.insert(id, (value.into(), source, detail));
    }

    /// The plain words of the field's value (a code).
    fn plain(&mut self, id: FieldId, plain: impl Into<String>) {
        self.plain.insert(id, plain.into());
    }

    fn ask(&mut self, field: FieldId, text: String, choices: Vec<Choice>) {
        self.issues.push(Issue {
            field,
            level: Level::Ask,
            text: mask_numbers(&text),
            choices,
        });
    }

    fn note(&mut self, field: FieldId, text: String) {
        self.issues.push(Issue {
            field,
            level: Level::Note,
            text: mask_numbers(&text),
            choices: Vec::new(),
        });
    }

    fn open(&self) -> bool {
        self.issues.iter().any(|i| i.level == Level::Ask)
    }

    fn views(&self) -> Vec<FieldView> {
        FieldId::ALL
            .into_iter()
            .filter_map(|id| {
                let (value, source, detail) = self.fields.get(&id)?.clone();
                let mine = self.issues.iter().filter(|i| i.field == id);
                let state = if mine.clone().any(|i| i.level == Level::Ask) {
                    FieldState::Ask
                } else if mine.count() > 0 {
                    FieldState::Note
                } else {
                    FieldState::Ok
                };
                // The folder is a path the instructor chose, shown as it is.
                let shown = |v: &str| {
                    if id == FieldId::Folder {
                        v.to_string()
                    } else {
                        mask_numbers(v)
                    }
                };
                Some(FieldView {
                    id,
                    plain: shown(self.plain.get(&id).unwrap_or(&value)),
                    value: shown(&value),
                    source,
                    detail: mask_numbers(&detail),
                    state,
                    fixed: self.fixed.contains(&id),
                })
            })
            .collect()
    }
}

fn choice(label: impl Into<String>, action: ChoiceAction) -> Choice {
    Choice {
        label: label.into(),
        action,
    }
}

fn answer(label: impl Into<String>, a: Answer) -> Choice {
    choice(label, ChoiceAction::Answer(a))
}

/// Asks which versions a key file's `labels` are, when the sheet cannot bubble them (`1, 2`);
/// `to`: the letters the one-click answer reads them as.
fn ask_numbered(
    card: &mut Card,
    name: &str,
    labels: &[String],
    printed: &[String],
    to: Option<Vec<String>>,
) {
    let mut choices = Vec::new();
    if let Some(to) = to {
        choices.push(answer(
            format!("Read {} as {}", join(labels), join(&to)),
            Answer::KeyVersions(to),
        ));
    }
    choices.push(choice(
        "Choose another key file…",
        ChoiceAction::Pick(FieldId::Key),
    ));
    card.ask(
        FieldId::Versions,
        format!(
            "{name} names its versions {}, which the sheet cannot bubble (it prints {}): which \
             versions are they?",
            join(labels),
            join(printed)
        ),
        choices,
    );
}

/// Where the key comes from.
enum Plan {
    /// The Remark import.
    Remark(RemarkImport),
    /// The key file and what it holds.
    File(KeyShape, String),
    /// Key sheets, named or found in the stack by their marker.
    Sheets,
    /// Inferred from the sheets, on the instructor's answer.
    Provisional,
    /// The key file cannot be read (asked already).
    Broken,
    /// Nothing gives a key (asked already).
    Missing,
}

/// What the key file `p` holds; `unlabelled`: its headerless rows that start with a letter are
/// read as answers alone ([`Answer::KeyRowsUnlabelled`]).
fn read_shape(p: &Path, printed: &PrintedSheet, unlabelled: bool) -> Result<KeyShape, AppError> {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "xlsx" | "xls" | "xlsm") {
        return Err(AppError::Input(
            "an Excel workbook: save the key as CSV (File › Save As › CSV UTF-8)".into(),
        ));
    }
    let bytes = std::fs::read(p)
        .map_err(|e| AppError::Input(format!("{e}; check the path of the file")))?;
    exam::key_file_shape_rows(
        &bytes,
        printed.questions,
        printed.options,
        unlabelled.then_some(false),
    )
}

/// The field a `define` error is about, with the choice that fixes it.
fn error_field(msg: &str) -> (FieldId, Vec<Choice>) {
    let m = msg.to_lowercase();
    let pick = |f: FieldId, what: &str| (f, vec![choice(what, ChoiceAction::Pick(f))]);
    if m.contains("roster") {
        pick(FieldId::Roster, "Choose another roster…")
    } else if m.contains("clo") || m.contains("mapping") || m.contains("objective") {
        pick(FieldId::Clo, "Choose another CLO file…")
    } else if m.contains("remark") {
        pick(FieldId::Remark, "Import another Remark export…")
    } else if m.contains("key") || m.contains("points") {
        pick(FieldId::Key, "Choose another key file…")
    } else if m.contains("multi-review") || m.contains("second review") {
        // chosen, never typed: its choices are the answers
        (
            FieldId::MultiReview,
            MULTI_REVIEW
                .iter()
                .map(|(c, p)| answer(*p, Answer::UseValue(FieldId::MultiReview, (*c).into())))
                .collect(),
        )
    } else if m.contains("cancel") {
        pick(FieldId::Sheet, "Choose the sheet's spec file…")
    } else if m.contains("id scheme") || m.contains("student-id") {
        (
            FieldId::IdScheme,
            vec![choice(
                "Type the scheme",
                ChoiceAction::Type(FieldId::IdScheme),
            )],
        )
    } else if m.contains("form") || m.contains("version") {
        (
            FieldId::Versions,
            vec![choice(
                "Type the versions",
                ChoiceAction::Type(FieldId::Versions),
            )],
        )
    } else if m.contains("question") {
        (
            FieldId::Questions,
            vec![choice(
                "Type the count",
                ChoiceAction::Type(FieldId::Questions),
            )],
        )
    } else {
        pick(FieldId::Sheet, "Choose the sheet…")
    }
}

/// Proposes every value of a new exam from what the instructor gave, the survey of the scans
/// and the exams folder ([`Proposal`]). A survey of other inputs (its request differs) is
/// ignored and the scans are to be surveyed again; without a survey the files alone give the
/// values, and grading's backstops hold what the survey would have asked about.
pub fn propose(given: &Given, survey: Option<&Survey>, ctx: &Context) -> Proposal {
    let t = &ctx.thresholds;
    let mut card = Card::default();
    let answers = &given.answers;
    let answered = |a: &Answer| answers.contains(a);
    let use_value = |f: FieldId| {
        answers.iter().rev().find_map(|a| match a {
            Answer::UseValue(g, v) if *g == f => Some(v.clone()),
            _ => None,
        })
    };
    let keep_typed = |f: FieldId| answered(&Answer::UseTypedValue(f));
    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let typed = &given.typed;

    // ---- Scans and the survey.
    let survey = if given.scans.is_empty() {
        card.ask(
            FieldId::Scans,
            "choose the scans to grade (PDF, TIFF, JPEG or PNG files, or their folder)".into(),
            vec![choice("Choose scans…", ChoiceAction::Pick(FieldId::Scans))],
        );
        None
    } else {
        match survey.map(|s| {
            survey::request_digest(
                &given.sheet,
                &given.scans,
                &given.key_sheets,
                given.roster.as_deref(),
            )
            .map(|d| (s, d == s.request))
        }) {
            None => None,
            Some(Ok((s, true))) => Some(s),
            Some(Ok((_, false))) => {
                card.ask(
                    FieldId::Scans,
                    "the scans were checked before the scans, key sheets, roster or sheet \
                     changed: check them again"
                        .into(),
                    vec![choice("Check the scans again", ChoiceAction::Survey)],
                );
                None
            }
            Some(Err(e)) => {
                card.ask(
                    FieldId::Scans,
                    e.to_string(),
                    vec![choice("Choose scans…", ChoiceAction::Pick(FieldId::Scans))],
                );
                None
            }
        }
    };
    let n = survey.map_or(0u64, |s| s.booklets as u64);
    let count = |x: &str| -> u32 {
        survey
            .and_then(|s| s.versions.by_letter.get(x))
            .copied()
            .unwrap_or(0)
    };
    let substantial = |c: u32| {
        n > 0
            && u64::from(c) >= u64::from(t.substantial_min).min(n)
            && share_at_least(c.into(), n, t.substantial_permille)
    };
    let pct = |c: u64| (c * 100 + n / 2).checked_div(n).unwrap_or(0);
    let scans_files = crate::ingest::collect_paths(&given.scans).map_or(0, |v| v.len());
    let mut scans_detail = Vec::new();
    if let Some(s) = survey {
        scans_detail.push(plural(s.booklets as u64, "student sheet", "student sheets"));
        if s.blank_booklets > 0 {
            scans_detail.push(format!("{} with no answer", s.blank_booklets));
        }
        if s.blank_pages > 0 {
            scans_detail.push(plural(s.blank_pages as u64, "blank page", "blank pages"));
        }
        if s.duplicates > 0 {
            scans_detail.push(plural(s.duplicates as u64, "duplicate", "duplicates"));
        }
        if s.refused > 0 {
            scans_detail.push(format!("{} unreadable", s.refused));
        }
        if s.photos > 0 {
            scans_detail.push(plural(s.photos as u64, "phone photo", "phone photos"));
        }
    } else if !given.scans.is_empty() {
        scans_detail.push("not checked yet".into());
    }
    card.set(
        FieldId::Scans,
        plural(scans_files as u64, "file", "files"),
        Source::Typed,
        scans_detail.join(" · "),
    );

    // ---- The sheet.
    let printed = match define::printed(&given.sheet) {
        Ok(p) => p,
        Err(e) => {
            card.ask(
                FieldId::Sheet,
                e.to_string(),
                vec![choice(
                    "Choose the sheet…",
                    ChoiceAction::Pick(FieldId::Sheet),
                )],
            );
            let folder = given
                .typed
                .folder
                .as_ref()
                .map_or_else(|| ctx.workspace.join("exam"), |d| ctx.workspace.join(d));
            return Proposal {
                fields: card.views(),
                issues: card.issues,
                inputs: None,
                key_sheets: given.key_sheets.clone(),
                folder,
            };
        }
    };
    let legacy = matches!(printed.template, TemplateChoice::Legacy(_));
    let (sheet_value, sheet_source) = match &given.sheet {
        Sheet::Template(t) => (t.clone(), Source::Template),
        Sheet::Spec(p) => (file_name(p), Source::SpecFile),
    };
    let mut sheet_detail = format!(
        "{} questions × {} options, versions {}",
        printed.questions,
        printed.options,
        join(&printed.versions)
    );
    if let Some(s) = survey {
        let ours = s.booklets + s.blank_booklets + s.key_sheets.len();
        let foreign = s.not_this_sheet.len();
        sheet_detail.push_str(&format!(
            " · {ours} of {} pages are this sheet",
            ours + foreign
        ));
        if foreign > 0 && ours * 2 < ours + foreign && !answered(&Answer::KeepSheet) {
            card.ask(
                FieldId::Sheet,
                format!(
                    "only {ours} of {} pages register as the {sheet_value} sheet: choose the \
                     sheet the scans were printed on",
                    ours + foreign
                ),
                vec![
                    choice("Choose the sheet…", ChoiceAction::Pick(FieldId::Sheet)),
                    answer(
                        format!("Grade anyway: hold the {foreign} other pages"),
                        Answer::KeepSheet,
                    ),
                ],
            );
        } else if foreign > 0 {
            let shown: Vec<String> = s
                .not_this_sheet
                .iter()
                .take(3)
                .map(|p| format!("{} ({})", p.label, p.code))
                .collect();
            card.note(
                FieldId::Sheet,
                format!(
                    "{} not this exam's sheet: {}{}; held for review, never graded",
                    plural(foreign as u64, "page is", "pages are"),
                    join(&shown),
                    if foreign > 3 { ", …" } else { "" }
                ),
            );
        }
    }
    card.set(
        FieldId::Sheet,
        sheet_value.clone(),
        sheet_source,
        sheet_detail,
    );

    // The letters the sheets bubble (printed ones), and the substantial ones.
    let bubbled: Vec<(String, u32)> = printed
        .versions
        .iter()
        .map(|x| (x.clone(), count(x)))
        .filter(|(_, c)| *c > 0)
        .collect();
    let subst: Vec<String> = bubbled
        .iter()
        .filter(|(_, c)| substantial(*c))
        .map(|(x, _)| x.clone())
        .collect();
    let counts_text = || -> String {
        let Some(s) = survey else {
            return String::new();
        };
        let mut v: Vec<String> = bubbled.iter().map(|(x, c)| format!("{x} {c}")).collect();
        if s.versions.blank > 0 {
            v.push(format!("blank {}", s.versions.blank));
        }
        if s.versions.unclear > 0 {
            v.push(format!("unclear {}", s.versions.unclear));
        }
        if v.is_empty() {
            "no sheet bubbles a version".into()
        } else {
            format!("sheets {}", v.join(" · "))
        }
    };
    // Key sheets the survey met that can give a key (whatever the versions).
    let seen_sheets: Vec<&survey::KeySheetSeen> = survey
        .map(|s| s.key_sheets.iter().collect())
        .unwrap_or_default();
    let readable_sheets: Vec<&survey::KeySheetSeen> = seen_sheets
        .iter()
        .copied()
        .filter(|k| k.problem.is_none())
        .collect();
    let sheet_letters: BTreeSet<String> = readable_sheets
        .iter()
        .filter(|k| printed.versions.contains(&k.version))
        .map(|k| k.version.clone())
        .collect();

    // ---- The key: where it comes from.
    let key_name = given.key.as_deref().map(file_name).unwrap_or_default();
    let plan = if let Some(imp) = &given.remark {
        if given.key.is_some() {
            card.ask(
                FieldId::Key,
                format!("the Remark import and {key_name} both give the key: use one of them"),
                vec![
                    choice("Use the Remark import", ChoiceAction::Clear(FieldId::Key)),
                    choice(
                        format!("Use {key_name}"),
                        ChoiceAction::Clear(FieldId::Remark),
                    ),
                ],
            );
        }
        if !given.key_sheets.is_empty() {
            card.ask(
                FieldId::KeySheets,
                "the Remark import and the key sheets both give the key: use one of them".into(),
                vec![
                    choice(
                        "Use the Remark import",
                        ChoiceAction::Clear(FieldId::KeySheets),
                    ),
                    choice("Use the key sheets", ChoiceAction::Clear(FieldId::Remark)),
                ],
            );
        }
        Plan::Remark(imp.clone())
    } else if let Some(k) = &given.key {
        match read_shape(k, &printed, answered(&Answer::KeyRowsUnlabelled)) {
            Ok(shape) => Plan::File(shape, key_name.clone()),
            Err(e) => {
                card.ask(
                    FieldId::Key,
                    format!("{key_name}: {e}"),
                    vec![choice(
                        "Choose another key file…",
                        ChoiceAction::Pick(FieldId::Key),
                    )],
                );
                Plan::Broken
            }
        }
    } else if (!given.key_sheets.is_empty() || !seen_sheets.is_empty())
        && !(survey.is_some() && readable_sheets.is_empty())
    {
        Plan::Sheets
    } else if answered(&Answer::InferKeyProvisionally) {
        Plan::Provisional
    } else {
        // the field's label says what is missing; key sheets that cannot give one say why
        let why = if seen_sheets.is_empty() {
            ""
        } else {
            "the key sheets cannot give a key (see the notes on them): "
        };
        card.ask(
            FieldId::Key,
            format!(
                "{why}choose the key file, import it from Remark, give the key sheets, or grade \
                 with a provisional key inferred from the answers (the results cannot be released \
                 until the real key replaces it)"
            ),
            vec![
                choice("Choose a key file…", ChoiceAction::Pick(FieldId::Key)),
                choice("Import from Remark…", ChoiceAction::Pick(FieldId::Remark)),
                choice("Key sheets…", ChoiceAction::Pick(FieldId::KeySheets)),
                answer(
                    "Grade with a provisional key (cannot be released)",
                    Answer::InferKeyProvisionally,
                ),
            ],
        );
        Plan::Missing
    };

    // ---- Versions (§3.1 of the design): typed, Remark, the key file, key sheets, sheets.
    let held: BTreeSet<String> = answers
        .iter()
        .filter_map(|a| match a {
            Answer::HoldVersions(v) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let key_versions_answer = answers.iter().rev().find_map(|a| match a {
        Answer::KeyVersions(v) => Some(v.clone()),
        _ => None,
    });
    let mut key_versions: Vec<(String, String)> = Vec::new();
    let typed_forms = use_value(FieldId::Versions)
        .map(|v| (v, Source::Answer))
        .or_else(|| text(&typed.forms).map(|v| (v, Source::Typed)))
        .and_then(|(v, s)| letters(&v).filter(|l| !l.is_empty()).map(|l| (l, s)));
    let printable = |v: &[String]| {
        v.iter().all(|x| printed.versions.contains(x))
            && v.iter().collect::<BTreeSet<_>>().len() == v.len()
    };
    let first_letters = |k: usize| printed.versions.iter().take(k).cloned().collect::<Vec<_>>();
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v.dedup();
        v
    };
    let (forms, forms_source, mut forms_detail): (Option<Vec<String>>, Source, String) =
        if let Some((v, s)) = typed_forms {
            // Typed versions are never replaced, but the key file is still read against them.
            let mut detail = String::new();
            if let Plan::File(shape, name) = &plan {
                match &shape.versions {
                    // Numbered versions are read as the letters answered.
                    KeyVersions::Named(labels) if !printable(labels) => {
                        let fits = key_versions_answer
                            .as_ref()
                            .filter(|kv| kv.len() == labels.len() && printable(kv));
                        if let Some(kv) = fits {
                            key_versions = labels
                                .iter()
                                .cloned()
                                .zip(kv.iter().cloned())
                                .filter(|(a, b)| a != b)
                                .collect();
                            detail =
                                format!("{name}'s versions {} read as {}", join(labels), join(kv));
                        } else {
                            let to = if labels.len() == v.len() && printable(&v) {
                                Some(v.clone())
                            } else {
                                (labels.len() <= printed.versions.len())
                                    .then(|| first_letters(labels.len()))
                            };
                            ask_numbered(&mut card, name, labels, &printed.versions, to);
                        }
                    }
                    // One answer column for several versions: never copied to each unasked.
                    KeyVersions::Shared
                        if v.len() >= 2 && !answered(&Answer::SameKeyForEveryVersion) =>
                    {
                        let c = counts_text();
                        card.ask(
                            FieldId::Versions,
                            format!(
                                "{name} has one answer column, but the exam declares {}{}: is it \
                                 the key of every version?",
                                join(&v),
                                if c.is_empty() {
                                    String::new()
                                } else {
                                    format!(" ({c})")
                                }
                            ),
                            vec![
                                choice(
                                    "Choose a key file with a column per version…",
                                    ChoiceAction::Pick(FieldId::Key),
                                ),
                                answer(
                                    "The same key for every version",
                                    Answer::SameKeyForEveryVersion,
                                ),
                            ],
                        );
                    }
                    _ => {}
                }
            }
            (Some(v), s, detail)
        } else {
            match &plan {
                Plan::Remark(imp) => (
                    Some(imp.forms()),
                    Source::RemarkImport,
                    format!("the Remark export has versions {}", join(&imp.forms())),
                ),
                Plan::File(shape, name) => match &shape.versions {
                    KeyVersions::Named(labels) => {
                        let fits = key_versions_answer
                            .as_ref()
                            .filter(|kv| kv.len() == labels.len() && printable(kv));
                        if let Some(kv) = fits {
                            key_versions = labels
                                .iter()
                                .cloned()
                                .zip(kv.iter().cloned())
                                .filter(|(a, b)| a != b)
                                .collect();
                            (
                                Some(sorted(kv.clone())),
                                Source::Answer,
                                format!("{name}'s versions {} read as {}", join(labels), join(kv)),
                            )
                        } else if !printable(labels) {
                            let k = labels.len();
                            let to = (k <= printed.versions.len()).then(|| first_letters(k));
                            ask_numbered(&mut card, name, labels, &printed.versions, to);
                            (None, Source::KeyFile, String::new())
                        } else {
                            if shape.first_cell_labels {
                                card.note(
                                    FieldId::Versions,
                                    format!(
                                        "{name} has no header row: the first cell of each row ({}) \
                                         is read as its version",
                                        join(labels)
                                    ),
                                );
                            }
                            (
                                Some(sorted(labels.clone())),
                                Source::KeyFile,
                                format!("{name} names {}", join(labels)),
                            )
                        }
                    }
                    KeyVersions::Shared => {
                        if subst.len() >= 2 && answered(&Answer::SameKeyForEveryVersion) {
                            (
                                Some(subst.clone()),
                                Source::Sheets,
                                format!(
                                    "{name} has one answer column: the same key for every \
                                     version the sheets bubble"
                                ),
                            )
                        } else if subst.len() >= 2 {
                            card.ask(
                                FieldId::Versions,
                                format!(
                                    "{name} has one answer column, but the {}: is it the key of \
                                     every version?",
                                    counts_text()
                                ),
                                vec![
                                    choice(
                                        "Choose a key file with a column per version…",
                                        ChoiceAction::Pick(FieldId::Key),
                                    ),
                                    answer(
                                        "The same key for every version",
                                        Answer::SameKeyForEveryVersion,
                                    ),
                                ],
                            );
                            (None, Source::KeyFile, String::new())
                        } else {
                            let one = subst.first().cloned();
                            let src = if one.is_some() {
                                Source::Sheets
                            } else {
                                Source::Default
                            };
                            (
                                Some(vec![one.unwrap_or_else(|| printed.versions[0].clone())]),
                                src,
                                format!("{name} has one answer column: one version"),
                            )
                        }
                    }
                    KeyVersions::Unlabeled(k) => {
                        let fits = key_versions_answer
                            .as_ref()
                            .filter(|kv| kv.len() == *k && printable(kv));
                        if let Some(kv) = fits {
                            (
                                Some(kv.clone()),
                                Source::Answer,
                                format!(
                                    "{name}'s {} read as {}",
                                    plural(*k as u64, "key", "unlabelled keys"),
                                    join(kv)
                                ),
                            )
                        } else if *k == 1 {
                            card.note(
                                FieldId::Versions,
                                format!(
                                    "{name}'s key names no version: it is read as version {}",
                                    printed.versions[0]
                                ),
                            );
                            (
                                Some(first_letters(1)),
                                Source::KeyFile,
                                format!("{name} has one key"),
                            )
                        } else {
                            let mut choices = Vec::new();
                            if *k <= printed.versions.len() {
                                let to = first_letters(*k);
                                choices.push(answer(
                                    format!("Read them as {}", join(&to)),
                                    Answer::KeyVersions(to),
                                ));
                            }
                            choices.push(choice(
                                "Choose another key file…",
                                ChoiceAction::Pick(FieldId::Key),
                            ));
                            card.ask(
                                FieldId::Versions,
                                format!(
                                    "{name} has {k} keys without a version label (add a header \
                                     row such as question,A,B): which versions are they, in \
                                     order?"
                                ),
                                choices,
                            );
                            (None, Source::KeyFile, String::new())
                        }
                    }
                },
                Plan::Sheets if survey.is_some() => {
                    let mut v: Vec<String> = sheet_letters
                        .iter()
                        .cloned()
                        .chain(subst.iter().filter(|x| !held.contains(*x)).cloned())
                        .collect();
                    if v.is_empty() {
                        v.push(printed.versions[0].clone());
                    }
                    for x in subst
                        .iter()
                        .filter(|x| !sheet_letters.contains(*x) && !held.contains(*x))
                    {
                        let c = count(x);
                        card.ask(
                            FieldId::Versions,
                            format!(
                                "{} bubble version {x}, but no key sheet is for version {x}",
                                plural(c, "sheet", "sheets")
                            ),
                            vec![
                                choice(
                                    "Add its key sheet…",
                                    ChoiceAction::Pick(FieldId::KeySheets),
                                ),
                                choice("Choose a key file…", ChoiceAction::Pick(FieldId::Key)),
                                answer(
                                    format!("Grade anyway: hold the {c} sheets"),
                                    Answer::HoldVersions(vec![x.clone()]),
                                ),
                            ],
                        );
                    }
                    (
                        Some(sorted(v)),
                        Source::KeySheets,
                        format!(
                            "key sheets for {}",
                            if sheet_letters.is_empty() {
                                "no version yet".to_string()
                            } else {
                                join(&sheet_letters.iter().cloned().collect::<Vec<_>>())
                            }
                        ),
                    )
                }
                Plan::Sheets => (
                    Some(first_letters(1)),
                    Source::Default,
                    "the key sheets' versions are read with the exam".into(),
                ),
                Plan::Provisional => {
                    let v: Vec<String> = bubbled
                        .iter()
                        .filter(|(x, c)| {
                            !held.contains(x)
                                && *c >= t.infer_min
                                && share_at_least((*c).into(), n, t.infer_permille)
                        })
                        .map(|(x, _)| x.clone())
                        .collect();
                    if v.is_empty() {
                        (
                            Some(first_letters(1)),
                            Source::Default,
                            "one version".into(),
                        )
                    } else {
                        (
                            Some(v),
                            Source::Sheets,
                            format!(
                                "versions bubbled on at least {} sheets get an inferred key",
                                t.infer_min
                            ),
                        )
                    }
                }
                Plan::Broken | Plan::Missing => {
                    let v: Vec<String> = subst
                        .iter()
                        .filter(|x| !held.contains(*x))
                        .cloned()
                        .collect();
                    if v.is_empty() {
                        (Some(first_letters(1)), Source::Default, String::new())
                    } else {
                        (Some(v), Source::Sheets, String::new())
                    }
                }
            }
        };
    // Every version has the one key of a one-column key file, whoever named the versions: a
    // sheet without a version is scored under it, never reviewed (CR-16), unless points per
    // version or key sheets in the stack (pending until confirmed) keep the versions apart.
    let same_key = matches!(&plan, Plan::File(shape, _)
        if shape.versions == KeyVersions::Shared && shape.points_per_version.is_empty())
        && seen_sheets.is_empty();
    // Whether the key source can give version `x` a key (a declared version needs one).
    let keyed = |x: &String| match &plan {
        Plan::File(shape, _) => match &shape.versions {
            KeyVersions::Shared => true,
            KeyVersions::Named(labels) => labels.iter().any(|l| {
                key_versions
                    .iter()
                    .find(|(a, _)| a == l)
                    .map_or(l, |(_, b)| b)
                    == x
            }),
            KeyVersions::Unlabeled(_) => false,
        },
        Plan::Remark(imp) => imp.forms().contains(x),
        Plan::Sheets | Plan::Provisional | Plan::Broken | Plan::Missing => true,
    };
    // C1–C5: the sheets against the versions.
    if let (Some(s), Some(f)) = (survey, forms.as_ref()) {
        let declared = |x: &String| f.contains(x);
        let named_by = match (&plan, forms_source) {
            (_, Source::Typed | Source::Answer) => "the exam".to_string(),
            (Plan::Remark(_), _) => "the Remark export".into(),
            (Plan::File(_, name), _) => name.clone(),
            (Plan::Sheets, _) => "the key sheets".into(),
            _ => "the exam".into(),
        };
        for (x, c) in &bubbled {
            if declared(x) {
                continue;
            }
            if held.contains(x) {
                card.note(
                    FieldId::Versions,
                    format!(
                        "{} bubbling version {x} {} held for your decision in Review, never \
                         scored under another version's key",
                        plural(*c, "sheet", "sheets"),
                        if *c == 1 { "is" } else { "are" }
                    ),
                );
            } else if substantial(*c) {
                let mut choices = Vec::new();
                if let Plan::File(..) = plan {
                    choices.push(choice(
                        "Choose another key file…",
                        ChoiceAction::Pick(FieldId::Key),
                    ));
                }
                if matches!(forms_source, Source::Typed | Source::Answer) && keyed(x) {
                    let mut more = f.clone();
                    more.push(x.clone());
                    more.sort();
                    choices.push(answer(
                        format!("Declare {}", join(&more)),
                        Answer::UseValue(FieldId::Versions, more.join(",")),
                    ));
                }
                choices.push(answer(
                    format!("Grade anyway: hold the {c} sheets"),
                    Answer::HoldVersions(vec![x.clone()]),
                ));
                if let Plan::File(shape, _) = &plan
                    && let KeyVersions::Named(labels) = &shape.versions
                    && forms_source != Source::Typed
                    && printable(labels)
                {
                    let as_ver = |l: &String| {
                        key_versions
                            .iter()
                            .find(|(a, _)| a == l)
                            .map_or_else(|| l.clone(), |(_, b)| b.clone())
                    };
                    for (i, l) in labels.iter().enumerate() {
                        let y = as_ver(l);
                        if count(&y) == 0 {
                            let mut kv: Vec<String> = labels.iter().map(as_ver).collect();
                            kv[i] = x.clone();
                            choices.push(answer(
                                format!("Read key version {y} as {x}"),
                                Answer::KeyVersions(kv),
                            ));
                        }
                    }
                }
                card.ask(
                    FieldId::Versions,
                    format!(
                        "{} bubble version {x}, but {named_by} has {}",
                        plural(*c, "sheet", "sheets"),
                        join(f)
                    ),
                    choices,
                );
            } else {
                card.note(
                    FieldId::Versions,
                    format!(
                        "{} version {x}, which the exam does not declare: held for your \
                         decision in Review, never scored under another version's key",
                        plural(*c, "sheet bubbles", "sheets bubble")
                    ),
                );
            }
        }
        let undeclared = s.versions.blank + s.versions.unclear;
        if f.len() >= 2 && !same_key && undeclared > 0 {
            card.note(
                FieldId::Versions,
                format!(
                    "{} the version blank or unclear: the version is inferred from the answers, \
                     then you confirm it in Review",
                    plural(undeclared, "sheet leaves", "sheets leave")
                ),
            );
        }
        if n > 0 {
            for x in f.iter().filter(|x| count(x) == 0) {
                card.note(FieldId::Versions, format!("no sheet bubbles version {x}"));
            }
        }
        for k in &readable_sheets {
            let usable = declared(&k.version) || (k.version.is_empty() && f.len() == 1);
            if usable {
                continue;
            }
            let what = if k.version.is_empty() || k.version == "?" {
                "a key sheet whose version is not bubbled clearly".to_string()
            } else {
                format!("a key sheet for version {}", k.version)
            };
            let mut choices = vec![choice(
                "Choose other scans…",
                ChoiceAction::Pick(FieldId::Scans),
            )];
            if matches!(forms_source, Source::Typed | Source::Answer)
                && printed.versions.contains(&k.version)
                && keyed(&k.version)
            {
                let mut more = f.clone();
                more.push(k.version.clone());
                more.sort();
                choices.insert(
                    0,
                    answer(
                        format!("Declare {}", join(&more)),
                        Answer::UseValue(FieldId::Versions, more.join(",")),
                    ),
                );
            } else if let Plan::File(..) = plan {
                choices.insert(
                    0,
                    choice("Choose another key file…", ChoiceAction::Pick(FieldId::Key)),
                );
            }
            card.ask(
                FieldId::KeySheets,
                format!(
                    "{} is {what}, but the exam declares {}: it would be refused and hold the \
                     whole run",
                    k.label,
                    join(f)
                ),
                choices,
            );
        }
        if let Plan::Remark(imp) = &plan {
            for v in &imp.versions {
                let c = u64::from(count(&v.form));
                if c.abs_diff(v.students as u64) > u64::from(t.remark_count_slack) {
                    card.note(
                        FieldId::Remark,
                        format!(
                            "the sheets show {c} version {} sheets, the Remark export {} \
                             students: are these the same scans?",
                            v.form, v.students
                        ),
                    );
                }
            }
        }
    }
    if let Some(f) = &forms {
        let c = counts_text();
        if !c.is_empty() {
            if !forms_detail.is_empty() {
                forms_detail.push_str(" · ");
            }
            forms_detail.push_str(&c);
        }
        card.set(FieldId::Versions, join(f), forms_source, forms_detail);
    } else {
        card.set(FieldId::Versions, "", forms_source, counts_text());
    }
    for k in seen_sheets.iter().filter_map(|k| k.problem.as_ref()) {
        card.note(FieldId::KeySheets, k.clone());
    }
    if let Some(s) = survey {
        for note in &s.key_sheet_notes {
            card.note(FieldId::KeySheets, note.clone());
        }
    }

    // ---- Questions (§3.2): typed, Remark, the key file, key sheets, the sheets.
    let ext = survey.map(|s| s.extent(t));
    let keep: BTreeSet<u16> = answers
        .iter()
        .filter_map(|a| match a {
            Answer::KeepQuestions(q) => Some(*q),
            _ => None,
        })
        .collect();
    let typed_q = text(&typed.questions).map(|q| {
        q.parse::<u16>()
            .ok()
            .filter(|q| (1..=printed.questions).contains(q))
            .ok_or(q)
    });
    let sheet_rows = readable_sheets
        .iter()
        .filter(|k| match &forms {
            Some(f) => f.contains(&k.version) || (k.version.is_empty() && f.len() == 1),
            None => true,
        })
        .map(|k| k.last_row)
        .max()
        .unwrap_or(0);
    let strong: Option<(u16, Source, String)> = match &plan {
        Plan::Remark(imp) => Some((
            imp.questions,
            Source::RemarkImport,
            format!("the Remark export has {} questions", imp.questions),
        )),
        Plan::File(shape, name) if shape.questions > 0 => {
            let q = shape.questions.min(u32::from(printed.questions)) as u16;
            let mut d = format!("{name} answers questions 1–{q}");
            if shape.listed > shape.questions {
                d.push_str(&format!(" (rows to {} are empty)", shape.listed));
            }
            Some((q, Source::KeyFile, d))
        }
        Plan::Sheets if sheet_rows > 0 => Some((
            sheet_rows,
            Source::KeySheets,
            format!("the key sheets mark rows 1–{sheet_rows}"),
        )),
        _ => None,
    };
    let ext_text = |e: &survey::Extent| -> String {
        let Some(s) = survey else {
            return String::new();
        };
        if n == 0 {
            return "no sheet has an answer".into();
        }
        let tail = if usize::from(e.n50) < s.rows.len() {
            format!(
                ", rows {}–{} on at most {} %",
                e.n50 + 1,
                s.rows.len(),
                e.tail_max_permille.div_ceil(10)
            )
        } else {
            String::new()
        };
        if e.n50 == 0 {
            format!(
                "sheets: no row is answered on half the sheets (the last marked row is {})",
                e.last
            )
        } else {
            format!(
                "sheets: rows 1–{} each answered on at least {} %{tail}",
                e.n50,
                pct(s.rows[0..usize::from(e.n50)]
                    .iter()
                    .map(|r| u64::from(r.marked))
                    .min()
                    .unwrap_or(0))
            )
        }
    };
    let questions: Option<(u16, Source, String)> = if let Some(q) =
        answers.iter().rev().find_map(|a| match a {
            Answer::UseQuestions(q) => Some(*q),
            _ => None,
        }) {
        Some((q, Source::Answer, String::new()))
    } else {
        match &typed_q {
            Some(Ok(q)) => Some((*q, Source::Typed, String::new())),
            Some(Err(q)) => {
                card.ask(
                    FieldId::Questions,
                    format!(
                        "`{q}` is not a question count: type a whole number from 1 to {}",
                        printed.questions
                    ),
                    vec![choice(
                        "Type the count",
                        ChoiceAction::Type(FieldId::Questions),
                    )],
                );
                None
            }
            None => match (&strong, &ext) {
                (Some(s), _) => Some(s.clone()),
                (None, Some(e)) if e.high && e.n50 > 0 => {
                    Some((e.n50, Source::Sheets, String::new()))
                }
                _ => {
                    let mut choices = Vec::new();
                    let mut why = "how many questions does the exam have?".to_string();
                    if let (Some(e), Some(s)) = (&ext, survey)
                        && n > 0
                    {
                        let from = usize::from(e.n50.max(1)).saturating_sub(3);
                        let to = (usize::from(e.n50) + 3).min(s.rows.len()).max(from + 1);
                        let shares: Vec<String> = s.rows[from..to]
                            .iter()
                            .map(|r| pct(r.marked.into()).to_string())
                            .collect();
                        why = format!(
                            "how many questions does the exam have? The sheets do not say \
                             clearly: rows {}–{to} are answered on {} % of the sheets",
                            from + 1,
                            shares.join(", ")
                        );
                        if e.n50 > 0 {
                            choices.push(answer(
                                format!("Use {}", e.n50),
                                Answer::UseQuestions(e.n50),
                            ));
                        }
                    }
                    choices.push(choice(
                        "Type the count",
                        ChoiceAction::Type(FieldId::Questions),
                    ));
                    card.ask(FieldId::Questions, why, choices);
                    None
                }
            },
        }
    };
    let nq = questions.as_ref().map(|q| q.0);
    if let Some((q, src, _)) = &questions {
        let (q, src) = (*q, *src);
        // Q1: a typed or answered count against the strong source.
        if let (Some((k, ssrc, _)), Source::Typed | Source::Answer) = (&strong, src)
            && *k != q
        {
            let k = *k;
            match ssrc {
                Source::RemarkImport => card.ask(
                    FieldId::Questions,
                    format!("the Remark export has {k} questions, not {q}"),
                    vec![answer(format!("Use {k}"), Answer::UseQuestions(k))],
                ),
                Source::KeyFile | Source::KeySheets if q < k && !keep.contains(&q) => {
                    let what = if *ssrc == Source::KeyFile {
                        format!("{key_name} answers questions 1–{k}")
                    } else {
                        format!("the key sheets mark rows 1–{k}")
                    };
                    card.ask(
                        FieldId::Questions,
                        format!("{what}, but the exam has {q} questions"),
                        vec![
                            answer(format!("Use {k}"), Answer::UseQuestions(k)),
                            answer(
                                format!("Keep {q}: questions {}–{k} are not graded", q + 1),
                                Answer::KeepQuestions(q),
                            ),
                        ],
                    );
                }
                Source::KeySheets if q > k => card.note(
                    FieldId::Questions,
                    format!(
                        "the key sheets mark rows up to {k}: rows {}–{q} get their key when you \
                         confirm the key sheets in Setup",
                        k + 1
                    ),
                ),
                _ => {}
            }
        }
        if let (Some(e), Some(s)) = (&ext, survey) {
            // Q2: students answered rows beyond the count.
            let beyond: Vec<usize> = (usize::from(q)..s.rows.len())
                .filter(|&r| {
                    let m = s.rows[r].marked;
                    m >= t.beyond_min && share_at_least(m.into(), n, t.beyond_permille)
                })
                .collect();
            if let Some(&last) = beyond.last()
                && src != Source::Sheets
                && !keep.contains(&q)
            {
                let m = (last + 1) as u16;
                let top = beyond.iter().map(|&r| s.rows[r].marked).max().unwrap_or(0);
                card.ask(
                    FieldId::Questions,
                    format!(
                        "rows {}–{m} are answered on up to {} % of the sheets, but the exam has \
                         {q} questions",
                        q + 1,
                        pct(top.into())
                    ),
                    vec![
                        answer(
                            format!("Use {m} (with a key that covers it)"),
                            Answer::UseQuestions(m),
                        ),
                        answer(
                            format!("Keep {q}: those rows are not part of the exam"),
                            Answer::KeepQuestions(q),
                        ),
                    ],
                );
            }
            // Q3: a count well past the sheets' extent.
            if e.high && q > e.n50 + t.speeded_rows {
                card.note(
                    FieldId::Questions,
                    format!(
                        "most sheets are answered up to row {} only, of {q} questions: a speeded \
                         end, or key rows beyond the questions asked",
                        e.n50
                    ),
                );
            }
            // Q6: two exams in one stack.
            if !answered(&Answer::OneExam) && n > 0 {
                let half = e.n50 / 2;
                let short: u32 = s
                    .last_rows
                    .iter()
                    .filter(|(r, _)| **r <= half)
                    .map(|(_, c)| *c)
                    .sum();
                let types: Vec<(&String, &u32)> = s
                    .exam_types
                    .iter()
                    .filter(|(_, c)| share_at_least(u64::from(**c), n, t.exam_type_permille))
                    .collect();
                let why = if half > 0
                    && short >= t.short_min
                    && share_at_least(short.into(), n, t.short_permille)
                {
                    Some(format!(
                        "{short} of {n} sheets end by row {half} while most run to row {}: the \
                         scans may hold two exams",
                        e.n50
                    ))
                } else if types.len() >= 2 {
                    Some(format!(
                        "the sheets bubble {}: the scans may hold two exams",
                        types
                            .iter()
                            .map(|(x, c)| format!("{x} on {c}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                } else {
                    None
                };
                if let Some(why) = why {
                    card.ask(
                        FieldId::Scans,
                        why,
                        vec![
                            answer("They are one exam", Answer::OneExam),
                            choice("Choose other scans…", ChoiceAction::Pick(FieldId::Scans)),
                        ],
                    );
                }
            }
        }
        // Q7: headerless key rows that start with a letter read as versions, while the exam has
        // more questions than they then answer: the letter may be each row's answer to
        // question 1. Asked, so the definition reads the rows as the card shows them.
        let mut rows_asked = false;
        if let Plan::File(shape, name) = &plan
            && let KeyVersions::Named(labels) = &shape.versions
            && shape.first_cell_labels
            && u32::from(q) > shape.questions
        {
            rows_asked = true;
            let mut choices = Vec::new();
            if shape.questions > 0 {
                choices.push(answer(
                    format!(
                        "The first cell is the version ({}): {} questions",
                        join(labels),
                        shape.questions
                    ),
                    Answer::UseQuestions(shape.questions as u16),
                ));
            }
            choices.push(answer(
                format!(
                    "The first cell is the answer to question 1: {} without a version",
                    plural(labels.len() as u64, "key", "keys")
                ),
                Answer::KeyRowsUnlabelled,
            ));
            choices.push(choice(
                "Choose another key file…",
                ChoiceAction::Pick(FieldId::Key),
            ));
            card.ask(
                FieldId::Key,
                format!(
                    "{name} has no header row: the first cell of each row ({}) is read as its \
                     version, so each row answers questions 1–{}, but the exam has {q} \
                     questions: is the first cell the version, or the answer to question 1?",
                    join(labels),
                    shape.questions
                ),
                choices,
            );
        }
        // Q4: key entries missing up to the count; Q5: key entries beyond the sheet.
        if let (Plan::File(shape, name), Some(f)) = (&plan, &forms)
            && !rows_asked
        {
            let label_version = |l: &str| -> Option<String> {
                if l.is_empty() {
                    return Some(String::new());
                }
                if let Some(i) = l.strip_prefix('#').and_then(|i| i.parse::<usize>().ok()) {
                    return f.get(i - 1).cloned();
                }
                let v = key_versions
                    .iter()
                    .find(|(a, _)| a == l)
                    .map_or_else(|| l.to_string(), |(_, b)| b.clone());
                f.contains(&v).then_some(v)
            };
            let mut gaps: BTreeMap<String, Vec<u32>> = BTreeMap::new();
            for (l, g) in shape.gaps.iter().filter(|(_, g)| *g <= u32::from(q)) {
                if let Some(v) = label_version(l) {
                    gaps.entry(v).or_default().push(*g);
                }
            }
            if u32::from(q) > shape.questions {
                let labels: Vec<String> = match &shape.versions {
                    KeyVersions::Shared => vec![String::new()],
                    _ => f.clone(),
                };
                for v in labels {
                    gaps.entry(v)
                        .or_default()
                        .extend(shape.questions + 1..=u32::from(q));
                }
            }
            if !gaps.is_empty() {
                let list = gaps
                    .iter()
                    .map(|(v, qs)| {
                        let r = exam::question_ranges(qs);
                        if v.is_empty() {
                            format!("question {r}")
                        } else {
                            format!("version {v} question {r}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if answered(&Answer::GradeWithKeyGaps) {
                    card.note(
                        FieldId::Key,
                        format!("{name} has no answer for {list}: scores wait until each is filled in or voided (*) in Setup"),
                    );
                } else {
                    let mut choices = vec![choice(
                        "Choose another key file…",
                        ChoiceAction::Pick(FieldId::Key),
                    )];
                    if u32::from(q) > shape.questions && shape.questions > 0 {
                        let k = shape.questions as u16;
                        choices.push(answer(
                            format!("Use {k} questions"),
                            Answer::UseQuestions(k),
                        ));
                    }
                    // A key file's empty columns (a template's spare versions) are no versions
                    // of the exam unless the instructor declares them.
                    let empty: Vec<&String> = f
                        .iter()
                        .filter(|v| gaps.get(*v).is_some_and(|g| g.len() == usize::from(q)))
                        .collect();
                    let rest: Vec<String> =
                        f.iter().filter(|v| !empty.contains(v)).cloned().collect();
                    if forms_source == Source::KeyFile
                        && matches!(shape.versions, KeyVersions::Named(_))
                        && !empty.is_empty()
                        && !rest.is_empty()
                    {
                        choices.push(answer(
                            format!("Use versions {}", join(&rest)),
                            Answer::UseValue(FieldId::Versions, rest.join(",")),
                        ));
                    }
                    choices.push(answer(
                        "Grade now; scores wait for the key",
                        Answer::GradeWithKeyGaps,
                    ));
                    card.ask(
                        FieldId::Key,
                        format!(
                            "{name} has no answer for {list}: nothing can be scored until each is \
                             filled in or voided (*)"
                        ),
                        choices,
                    );
                }
            }
            if !shape.beyond_sheet.is_empty() && !keep.contains(&q) {
                card.ask(
                    FieldId::Key,
                    format!(
                        "{name} answers questions {}, but the sheet prints {}",
                        exam::question_ranges(&shape.beyond_sheet),
                        printed.questions
                    ),
                    vec![
                        choice("Choose another key file…", ChoiceAction::Pick(FieldId::Key)),
                        answer(
                            format!("Keep {q}: the questions beyond are not graded"),
                            Answer::KeepQuestions(q),
                        ),
                    ],
                );
            }
        }
    }
    if let Some((q, src, detail)) = &questions {
        let mut d = detail.clone();
        if d.is_empty()
            && let Some((_, _, sd)) = &strong
        {
            d = sd.clone();
        }
        if let Some(e) = &ext {
            let x = ext_text(e);
            if !x.is_empty() {
                if !d.is_empty() {
                    d.push_str(" · ");
                }
                d.push_str(&x);
            }
        }
        card.set(FieldId::Questions, q.to_string(), *src, d);
    } else {
        card.set(
            FieldId::Questions,
            "",
            Source::Default,
            ext.as_ref().map(&ext_text).unwrap_or_default(),
        );
    }

    // ---- Options (§3.3): typed, else printed; never lowered automatically.
    let typed_o = use_value(FieldId::Options)
        .map(|v| (v, Source::Answer))
        .or_else(|| text(&typed.options).map(|v| (v, Source::Typed)));
    let options = match typed_o {
        Some((v, src)) => match v
            .parse::<u8>()
            .ok()
            .filter(|o| (2..=printed.options).contains(o))
        {
            Some(o) => Some((o, src)),
            None => {
                card.ask(
                    FieldId::Options,
                    format!(
                        "`{v}` options: type a number from 2 to {} (the sheet prints {})",
                        printed.options, printed.options
                    ),
                    vec![
                        answer(
                            format!("Use {}", printed.options),
                            Answer::UseValue(FieldId::Options, printed.options.to_string()),
                        ),
                        choice("Type the options", ChoiceAction::Type(FieldId::Options)),
                    ],
                );
                None
            }
        },
        None => Some((
            printed.options,
            if legacy {
                Source::Template
            } else {
                Source::SpecFile
            },
        )),
    };
    let option_letter = |o: u8| char::from(b'A' + o.saturating_sub(1)).to_string();
    if let Some((o, src)) = options {
        let mut d = vec![format!("A–{} printed", option_letter(printed.options))];
        if let Plan::File(shape, name) = &plan
            && let Some(h) = shape.highest_option
        {
            d.push(format!("{name} uses A–{}", option_letter(h)));
            if h > o {
                card.ask(
                    FieldId::Options,
                    format!(
                        "{name} uses option {}, but the exam has {o} options",
                        option_letter(h)
                    ),
                    vec![answer(
                        format!("Use {}", printed.options),
                        Answer::UseValue(FieldId::Options, printed.options.to_string()),
                    )],
                );
            }
        }
        if let (Some(s), Some(q)) = (survey, nq)
            && o < printed.options
        {
            let marks: u32 = s
                .rows
                .iter()
                .take(usize::from(q))
                .flat_map(|r| r.by_option.iter().skip(usize::from(o)))
                .sum();
            if marks > 0 {
                card.note(
                    FieldId::Options,
                    format!(
                        "{} on options beyond {} go to review (MRK-017)",
                        plural(marks, "mark", "marks"),
                        option_letter(o)
                    ),
                );
            }
        }
        card.set(FieldId::Options, o.to_string(), src, d.join(" · "));
    }

    // ---- Cancel mode (§3.5): the sheet's own convention. The printed sheet fixes it: the card
    // shows it and takes nothing typed (the command line's typed mode must agree).
    card.fixed.insert(FieldId::CancelMode);
    let typed_c = use_value(FieldId::CancelMode)
        .map(|v| (v, Source::Answer))
        .or_else(|| text(&typed.cancel_mode).map(|v| (v, Source::Typed)));
    let mut cancel_input = None;
    match printed.cancel_mode {
        Some(m) => {
            let name = cancel_name(m);
            let (plain, meaning) = cancel_plain(m);
            let (detail, src) = if legacy {
                (
                    format!("{meaning}: the only convention of this sheet"),
                    Source::Template,
                )
            } else {
                (
                    format!("{meaning}: printed on the sheets ({sheet_value})"),
                    Source::SpecFile,
                )
            };
            match typed_c {
                Some((v, s)) if v != name => {
                    card.ask(
                        FieldId::CancelMode,
                        format!(
                            "the sheets print the cancel mode {name} ({}), not {v}",
                            plain.to_lowercase()
                        ),
                        vec![answer(
                            format!("Use {name}"),
                            Answer::UseValue(FieldId::CancelMode, name.into()),
                        )],
                    );
                    if let Some(p) = cancel_plain_of(&v) {
                        card.plain(FieldId::CancelMode, p);
                    }
                    card.set(FieldId::CancelMode, v, s, detail);
                }
                Some((v, s)) => {
                    cancel_input = Some(v.clone());
                    card.plain(FieldId::CancelMode, plain);
                    card.set(FieldId::CancelMode, v, s, detail);
                }
                None => {
                    card.plain(FieldId::CancelMode, plain);
                    card.set(FieldId::CancelMode, name, src, detail);
                }
            }
        }
        None => {
            card.ask(
                FieldId::CancelMode,
                format!(
                    "{sheet_value} sets no cancel_mode, so the marking convention printed on the \
                     sheets is unknown: set it in the spec to the one the sheets print"
                ),
                vec![choice(
                    "Choose the sheet's spec file…",
                    ChoiceAction::Pick(FieldId::Sheet),
                )],
            );
            card.set(FieldId::CancelMode, "", Source::SpecFile, String::new());
        }
    }

    // ---- Student-ID scheme (§3.6).
    let default_scheme = scheme_name(&if legacy {
        exam::IdSchemeChoice::Iau
    } else {
        exam::IdSchemeChoice::for_grid(printed.id_digits)
    });
    let (scheme, scheme_src) = use_value(FieldId::IdScheme)
        .map(|v| (v, Source::Answer))
        .or_else(|| text(&typed.id_scheme).map(|v| (v, Source::Typed)))
        .unwrap_or_else(|| {
            (
                default_scheme.clone(),
                if legacy {
                    Source::Template
                } else {
                    Source::SpecFile
                },
            )
        });
    let mut scheme_detail = format!("{} digits on the sheet", printed.id_digits);
    if let Some(s) = survey
        && s.ids.complete > 0
    {
        let complete = s.ids.complete;
        let fit = |name: &str| s.ids.fit.get(name).copied().unwrap_or(0);
        let ours = fit(&scheme);
        scheme_detail.push_str(&format!(" · {ours} of {complete} complete numbers fit"));
        let misfit = !share_at_least(ours.into(), complete.into(), t.id_misfit_permille);
        let theirs = share_at_least(
            fit(&default_scheme).into(),
            complete.into(),
            t.id_fit_permille,
        );
        if scheme != default_scheme && misfit && theirs && !keep_typed(FieldId::IdScheme) {
            // the scheme by its plain words, its code (what is typed) beside them
            let named = |c: &str| match scheme_plain(c) {
                Some(p) => format!("{p} ({c})"),
                None => c.to_string(),
            };
            card.ask(
                FieldId::IdScheme,
                format!(
                    "only {ours} of {complete} student numbers fit {}; {} fit {}",
                    named(&scheme),
                    fit(&default_scheme),
                    named(&default_scheme)
                ),
                vec![
                    answer(
                        format!("Use {}", named(&default_scheme)),
                        Answer::UseValue(FieldId::IdScheme, default_scheme.clone()),
                    ),
                    answer(
                        format!("Keep {}", named(&scheme)),
                        Answer::UseTypedValue(FieldId::IdScheme),
                    ),
                ],
            );
        } else if !share_at_least(ours.into(), complete.into(), t.id_fit_permille) {
            card.note(
                FieldId::IdScheme,
                format!(
                    "{} of {complete} complete student numbers do not fit {scheme}: those sheets \
                     go to identity review",
                    complete - ours
                ),
            );
        }
    }
    if let Some(p) = scheme_plain(&scheme) {
        card.plain(FieldId::IdScheme, p);
    }
    card.set(FieldId::IdScheme, scheme.clone(), scheme_src, scheme_detail);

    // ---- Second review and high stakes: never and off unless chosen (§3.7).
    let multi_src = use_value(FieldId::MultiReview)
        .map(|v| (v, Source::Answer))
        .or_else(|| text(&typed.multi_review).map(|v| (v, Source::Typed)));
    let plain_multi = |v: &str| MULTI_REVIEW.iter().find(|(c, _)| *c == v).map(|(_, p)| *p);
    if let Some((v, _)) = &multi_src
        && plain_multi(v).is_none()
    {
        card.ask(
            FieldId::MultiReview,
            format!(
                "`{v}` is not a second review: choose {}",
                MULTI_REVIEW
                    .iter()
                    .map(|(_, p)| p.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            MULTI_REVIEW
                .iter()
                .map(|(c, p)| answer(*p, Answer::UseValue(FieldId::MultiReview, (*c).into())))
                .collect(),
        );
    }
    let (multi, multi_from) = multi_src.unwrap_or_else(|| ("never".into(), Source::Default));
    let doubles = survey.map(|s| s.multi_marks).unwrap_or(0);
    let multi_detail = match multi.as_str() {
        "always" => "every double mark goes to review".to_string(),
        "high_stakes" if typed.high_stakes == Some(true) => {
            "this exam is high-stakes: every double mark goes to review".into()
        }
        "high_stakes" => "this exam is not high-stakes: confident double marks are scored as \
                          wrong without review"
            .into(),
        _ => "confident double marks are scored as wrong without review".into(),
    };
    if let Some(p) = plain_multi(&multi) {
        card.plain(FieldId::MultiReview, p);
    }
    card.set(
        FieldId::MultiReview,
        multi.clone(),
        multi_from,
        if survey.is_some() {
            format!("{multi_detail} ({doubles} on these sheets)")
        } else {
            multi_detail
        },
    );
    let high_stakes = typed.high_stakes.unwrap_or(false);
    card.set(
        FieldId::HighStakes,
        if high_stakes { "on" } else { "off" },
        if typed.high_stakes.is_some() {
            Source::Typed
        } else {
            Source::Default
        },
        "when on, stricter margins and erasures go to review (more items, never fewer)".into(),
    );

    // ---- Title, course, term (§3.8): labels only, harmless to scores.
    let folder_name = scans_folder(&given.scans);
    let folder_course = folder_name.as_deref().and_then(course_in);
    let title = if let Some(v) = text(&typed.title) {
        (v, Source::Typed, String::new())
    } else if let Some((x, c)) = survey
        .and_then(|s| {
            s.exam_types
                .iter()
                .max_by_key(|(x, c)| (**c, std::cmp::Reverse(x.as_str())))
        })
        .filter(|(_, c)| n > 0 && share_at_least(u64::from(**c), n, t.title_permille))
    {
        (
            x.clone(),
            Source::Sheets,
            format!("exam-type bubble {x} on {c} of {n} sheets"),
        )
    } else if let Some(name) = folder_name
        .as_deref()
        .filter(|n| !GENERIC_FOLDERS.contains(&n.to_lowercase().as_str()))
    {
        let rest = match &folder_course {
            Some((_, r)) => format!("{}{}", &name[..r.start], &name[r.end..]),
            None => name.to_string(),
        };
        let rest = rest
            .trim_matches(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .replace('_', " ");
        if rest.is_empty() {
            ("Exam".into(), Source::Default, String::new())
        } else {
            (
                rest,
                Source::Folder,
                format!("the scans' folder \"{name}\""),
            )
        }
    } else {
        ("Exam".into(), Source::Default, String::new())
    };
    card.set(FieldId::Title, title.0.clone(), title.1, title.2);
    let course = if let Some(v) = text(&typed.course) {
        (v, Source::Typed, String::new())
    } else if let Some((c, _)) = &folder_course {
        (
            c.clone(),
            Source::Folder,
            format!(
                "the scans' folder \"{}\"; edit it if wrong",
                folder_name.clone().unwrap_or_default()
            ),
        )
    } else {
        (String::new(), Source::Default, String::new())
    };
    card.set(FieldId::Course, course.0.clone(), course.1, course.2);
    let now = days(&ctx.now_utc);
    let term = if let Some(v) = text(&typed.term) {
        (v, Source::Typed, String::new())
    } else if let Some((r, ago)) = ctx
        .recent
        .iter()
        .filter(|r| !r.term.trim().is_empty())
        .filter_map(|r| Some((r, now? - days(&r.graded_utc)?)))
        .filter(|(_, ago)| (0..=t.term_days).contains(ago))
        .min_by_key(|(r, ago)| (*ago, std::cmp::Reverse(r.graded_utc.clone())))
    {
        (
            r.term.clone(),
            Source::Workspace,
            format!(
                "the last exam graded in this exams folder{}, {ago} days ago",
                if r.course.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", r.course)
                }
            ),
        )
    } else {
        (String::new(), Source::Default, String::new())
    };
    card.set(FieldId::Term, term.0.clone(), term.1, term.2);

    // ---- Key, roster and CLO files.
    let key_value = match &plan {
        Plan::Remark(_) => "Remark import".to_string(),
        Plan::File(_, name) => name.clone(),
        Plan::Sheets => plural(
            seen_sheets.len().max(given.key_sheets.len()) as u64,
            "key sheet",
            "key sheets",
        ),
        Plan::Provisional => "provisional (inferred from the answers)".into(),
        Plan::Broken => key_name.clone(),
        Plan::Missing => String::new(),
    };
    let (key_src, key_detail) = match &plan {
        Plan::Remark(imp) => (
            Source::RemarkImport,
            format!(
                "{} × {} questions with exact points",
                plural(imp.versions.len() as u64, "version", "versions"),
                imp.questions
            ),
        ),
        Plan::File(shape, _) => (
            Source::KeyFile,
            format!(
                "{}{}",
                match &shape.versions {
                    KeyVersions::Named(v) => plural(v.len() as u64, "version", "versions"),
                    KeyVersions::Shared => "one key for every version".into(),
                    KeyVersions::Unlabeled(k) =>
                        plural(*k as u64, "unlabelled key", "unlabelled keys"),
                },
                if shape.points || !shape.points_per_version.is_empty() {
                    " · points column"
                } else {
                    " · 1 point each"
                }
            ),
        ),
        Plan::Sheets => {
            let shown: Vec<String> = seen_sheets
                .iter()
                .map(|k| {
                    let v = if k.version.is_empty() {
                        "?"
                    } else {
                        &k.version
                    };
                    format!("{v}: {}", k.label)
                })
                .collect();
            (
                Source::KeySheets,
                format!(
                    "{}nothing is scored until you confirm them in Setup · a student number of all \
                     0 or all 9 marks a key sheet",
                    if shown.is_empty() {
                        String::new()
                    } else {
                        format!("{} · ", join(&shown))
                    }
                ),
            )
        }
        Plan::Provisional => (
            Source::Answer,
            "the most common answers of each version: the results stay provisional and cannot be \
             released until you replace the key in Setup"
                .into(),
        ),
        Plan::Broken | Plan::Missing => (Source::Default, String::new()),
    };
    card.set(FieldId::Key, key_value, key_src, key_detail);
    let given_by = match &plan {
        Plan::File(..) => Some(key_name.clone()),
        Plan::Remark(_) => Some("the Remark import".to_string()),
        _ => None,
    };
    if let Some(by) = given_by
        && !seen_sheets.is_empty()
    {
        card.note(
            FieldId::KeySheets,
            format!(
                "{} also in the scans: {by} gives the key, the key sheets are laid over it and \
                 the run waits until you confirm them in Setup",
                plural(seen_sheets.len() as u64, "key sheet is", "key sheets are")
            ),
        );
    }
    if !given.key_sheets.is_empty() || !seen_sheets.is_empty() {
        let named: Vec<String> = given.key_sheets.iter().map(|p| file_name(p)).collect();
        card.set(
            FieldId::KeySheets,
            if named.is_empty() {
                plural(
                    seen_sheets.len() as u64,
                    "found in the scans",
                    "found in the scans",
                )
            } else {
                join(&named)
            },
            if named.is_empty() {
                Source::Sheets
            } else {
                Source::Typed
            },
            String::new(),
        );
    }
    if let Some(imp) = &given.remark {
        card.set(
            FieldId::Remark,
            plural(imp.workbooks.len() as u64, "workbook", "workbooks"),
            Source::Typed,
            imp.workbooks
                .iter()
                .map(|w| w.file.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    card.set(
        FieldId::Roster,
        given.roster.as_deref().map(file_name).unwrap_or_default(),
        if given.roster.is_some() {
            Source::Typed
        } else {
            Source::Default
        },
        if given.roster.is_some() {
            String::new()
        } else {
            "none: students are named by the numbers on their sheets and there is no Blackboard \
             upload; add a roster later in Setup"
                .into()
        },
    );
    card.set(
        FieldId::Clo,
        given.clo.as_deref().map(file_name).unwrap_or_default(),
        if given.clo.is_some() {
            Source::Typed
        } else {
            Source::Default
        },
        String::new(),
    );

    // ---- The folder and the inputs, checked by `define`.
    let slug_folder = |title: &str, course: &str| {
        let mut d = ExamDefinition::new(
            title,
            TemplateChoice::Legacy("iau-ccsit-v1".into()),
            1,
            5,
            vec!["A".into()],
            CancelMode::EraseOnly,
        );
        d.course_code = course.to_string();
        define::new_exam_dir(&ctx.workspace, &d)
    };
    // a typed name (not a whole path) is a folder below the exams folder
    let typed_folder = typed.folder.as_ref().map(|d| ctx.workspace.join(d));
    if let Some(dir) = &typed_folder
        && (dir.join("project.omrproj").exists() || dir.extension().is_some_and(|e| e == "omrproj"))
    {
        card.ask(
            FieldId::Folder,
            format!(
                "{} already holds an exam: choose a new folder",
                dir.display()
            ),
            vec![choice(
                "Choose another folder…",
                ChoiceAction::Pick(FieldId::Folder),
            )],
        );
    }
    let mut folder = typed_folder
        .clone()
        .unwrap_or_else(|| slug_folder(&title.0, &course.0));
    let mut inputs = None;
    if !card.open()
        && let (Some(f), Some(q), Some((o, _))) = (forms.clone(), nq, options)
    {
        let mut i = Inputs::new(given.sheet.clone(), q);
        i.options = Some(o);
        i.forms = Some(f);
        i.key_versions = key_versions.clone();
        // Headerless rows of one version each are read as the card shows them.
        if let Plan::File(shape, _) = &plan {
            i.key_rows_labelled = match shape.versions {
                KeyVersions::Named(_) => Some(true),
                KeyVersions::Unlabeled(_) => Some(false),
                KeyVersions::Shared => None,
            };
        }
        match &plan {
            Plan::Remark(imp) => i.remark = Some(Box::new(imp.clone())),
            Plan::File(..) => i.key.clone_from(&given.key),
            Plan::Provisional => i.infer_key = true,
            _ => {}
        }
        i.roster.clone_from(&given.roster);
        i.clo.clone_from(&given.clo);
        i.title = Some(title.0.clone());
        i.course = Some(course.0.clone()).filter(|c| !c.is_empty());
        i.term = Some(term.0.clone()).filter(|c| !c.is_empty());
        i.cancel_mode = cancel_input;
        i.multi_review = Some(multi.clone());
        i.high_stakes = high_stakes;
        i.id_scheme = Some(scheme);
        i.provenance = card
            .views()
            .into_iter()
            .filter(|v| v.id != FieldId::Folder)
            .map(|v| SetupField {
                field: v.id.name().into(),
                value: v.value,
                source: v.source.text().into(),
                detail: v.detail,
            })
            .collect();
        match define::define(&i) {
            Ok((def, warnings)) => {
                for w in warnings {
                    let field = if w.starts_with("key file") {
                        FieldId::Key
                    } else {
                        FieldId::Roster
                    };
                    card.note(field, w);
                }
                if typed_folder.is_none() {
                    folder = define::new_exam_dir(&ctx.workspace, &def);
                }
                inputs = Some(i);
            }
            Err(e) => {
                let msg = e.to_string();
                let (field, choices) = error_field(&msg);
                card.ask(field, msg, choices);
            }
        }
    }
    card.set(
        FieldId::Folder,
        folder.display().to_string(),
        if typed_folder.is_some() {
            Source::Typed
        } else {
            Source::Workspace
        },
        "a new folder for the exam and its exports".into(),
    );
    Proposal {
        fields: card.views(),
        issues: card.issues,
        inputs,
        key_sheets: given.key_sheets.clone(),
        folder,
    }
}
