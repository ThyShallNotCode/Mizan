//! The survey of a stack of scans before its exam is defined (G-3): the stack is read once, in
//! memory, under a survey definition that decides every printed row, declares every printed
//! version and finds every key sheet, and only counts are kept ([`Survey`]): which versions the
//! students bubbled, how far they answered, which key sheets the stack holds and how many pages
//! are this sheet at all. The New-exam proposal ([`crate::propose`]) takes its evidence from it.
//!
//! The survey grades in memory ([`pipeline::grade_with_key_sheets`] without a project folder,
//! as [`crate::keysheet::read_key_sheets`] does), so nothing is written and recognition stays
//! key-blind (R2). No student number is kept: the session is dropped once counted, key sheets
//! and foreign pages are named by their page (`scans.pdf page 37`), and every text taken from
//! the run is masked ([`crate::export::mask_numbers`]). Like a grading run, its counts do not
//! depend on the input order or the thread count (A5).

use crate::AppError;
use crate::define::{self, Inputs, Sheet};
use crate::exam::{ExamDefinition, IdSchemeChoice, TemplateChoice};
use crate::export::mask_numbers;
use crate::model::{PageStatus, Session};
use crate::pipeline::{self, Cancel, GradeOptions, Progress};
use crate::propose::Thresholds;
use omr_model::Response;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What to survey.
#[derive(Clone, Debug, PartialEq)]
pub struct SurveyRequest {
    /// The printed sheet.
    pub sheet: Sheet,
    /// Scan files and folders.
    pub scans: Vec<PathBuf>,
    /// Files named as key sheets (every sheet in them is a key sheet, §10.1).
    pub key_sheets: Vec<PathBuf>,
    /// The roster, used only to tell whether a key-sheet marker could be a student's number
    /// (a roster that cannot be read is left out here; defining the exam reports it).
    pub roster: Option<PathBuf>,
    /// Worker threads (0 = available parallelism).
    pub threads: usize,
    /// The operator's cancel (the New-exam form discards a survey of scans it no longer
    /// shows): the survey stops between its stages ([`crate::AppError::Cancelled`]).
    pub cancel: Cancel,
}

/// Version bubbles of the students' sheets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCounts {
    /// Confident letters (own sheets: the verified page code's form) → sheets.
    pub by_letter: BTreeMap<String, u32>,
    /// Sheets that leave the version blank.
    pub blank: u32,
    /// Sheets whose version is not read clearly (two letters, a light mark).
    pub unclear: u32,
}

/// How the students' sheets use one printed row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowUse {
    /// Sheets that mark the row (one option or several; accepted or only proposed).
    pub marked: u32,
    /// Sheets that mark several options on it.
    pub multi: u32,
    /// Marks per option.
    pub by_option: Vec<u32>,
}

/// How far the students answered ([`Survey::extent`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    /// The last row marked on at least half the sheets (N50; 0: none).
    pub n50: u16,
    /// The last row any sheet marks.
    pub last: u16,
    /// The most-marked row beyond N50, as a share of the sheets (‰).
    pub tail_max_permille: u32,
    /// The rows up to N50 are answered on most sheets and almost none beyond it: N50 is the
    /// exam's length as far as the sheets can tell ([`Thresholds`]).
    pub high: bool,
}

/// Student numbers as read, counted (never kept).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdCounts {
    /// Sheets whose every number column is a confident digit.
    pub complete: u32,
    /// Of those, the numbers each student-ID scheme accepts (`iau`, `pnu`, `ksu`, `digits:N`).
    pub fit: BTreeMap<String, u32>,
}

/// A key sheet the survey met (named, or found in the stack by its marker). Labels are
/// masked like every text of the survey: a file named by a student number never shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySheetSeen {
    /// Where it is: its first page (`scans.pdf page 37`).
    pub label: String,
    /// Named as a key sheet (else found by its marker).
    pub named: bool,
    /// The marker digit its student-number grid shows.
    pub marker: Option<u8>,
    /// Its version: a letter, `` blank, `?` unclear.
    pub version: String,
    /// The last row it marks (0: none).
    pub last_row: u16,
    /// Why it cannot give a key whatever the exam's versions (not readable, not registered,
    /// no marker), masked; a blank or unclear version is judged against the exam's versions.
    pub problem: Option<String>,
}

/// A page that is not a student's sheet of this exam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageNote {
    /// Where it is (`scans.pdf page 3`).
    pub label: String,
    /// Its finding (`REG-001`, `IDN-002`, `CAP-001` …).
    pub code: String,
}

/// What a stack of scans shows, counts only ([`survey`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Survey {
    /// SHA-384 (hex) over the sorted SHA-384s of the files read, the template revision, the
    /// named key-sheet files and the roster: the content surveyed.
    pub fingerprint: String,
    /// The request ([`request_digest`]): a proposal ignores a survey of other inputs.
    pub request: String,
    /// The template (`iau-ccsit-v1`, `gen:40q-2f`).
    pub template: String,
    /// Input files (duplicates and unreadable files included).
    pub files: usize,
    /// Pages.
    pub pages: usize,
    /// Students' sheets with at least one marked row (n, the base of every share).
    pub booklets: usize,
    /// Students' sheets with no marked row.
    pub blank_booklets: usize,
    /// Blank backs and bleed-through pages (CAP-012).
    pub blank_pages: usize,
    /// Byte-identical duplicates.
    pub duplicates: usize,
    /// Pages refused at capture (unreadable, CAP-001/CAP-004).
    pub refused: usize,
    /// Students' pages that are phone photos.
    pub photos: usize,
    /// Pages that do not register as this sheet, or are another exam's (REG-001, IDN-002).
    pub not_this_sheet: Vec<PageNote>,
    /// The students' version bubbles.
    pub versions: VersionCounts,
    /// Every printed row, in order.
    pub rows: Vec<RowUse>,
    /// Booklets by their last marked row (1-based).
    pub last_rows: BTreeMap<u16, u32>,
    /// Confident double marks (scored as wrong unless reviewed).
    pub multi_marks: u32,
    /// Student numbers.
    pub ids: IdCounts,
    /// Key sheets, in page order.
    pub key_sheets: Vec<KeySheetSeen>,
    /// Notes on the key-sheet rule (a marker that could be a student's number), masked.
    pub key_sheet_notes: Vec<String>,
    /// Exam-type bubbles (IAU sheet: `Midterm`, `Quiz 2` …) → sheets.
    pub exam_types: BTreeMap<String, u32>,
    /// Section bubbles → sheets.
    pub sections: BTreeMap<String, u32>,
}

/// True when `count` of `n` is at least `permille` ‰.
pub(crate) fn share_at_least(count: u64, n: u64, permille: u32) -> bool {
    count * 1000 >= u64::from(permille) * n
}

impl Survey {
    /// How far the students answered, with the extent rule of `t`: N50 is the last row marked
    /// on at least half the sheets; the extent is high when that row is marked on at least
    /// 70 % of them, every row before it on at least 30 % and no row after it on more than 5 %.
    pub fn extent(&self, t: &Thresholds) -> Extent {
        let n = self.booklets as u64;
        let at_least = |q: usize, p: u32| n > 0 && share_at_least(self.rows[q].marked.into(), n, p);
        let n50 = (0..self.rows.len())
            .rev()
            .find(|&q| at_least(q, t.n50_permille))
            .map_or(0, |q| q + 1);
        let last = self
            .rows
            .iter()
            .rposition(|r| r.marked > 0)
            .map_or(0, |q| q + 1);
        let tail = self.rows[n50..].iter().map(|r| r.marked).max().unwrap_or(0);
        let high = n50 > 0
            && at_least(n50 - 1, t.high_at_n50_permille)
            && u64::from(tail) * 1000 <= u64::from(t.high_tail_permille) * n
            && (0..n50).all(|q| at_least(q, t.high_body_permille));
        Extent {
            n50: n50 as u16,
            last: last as u16,
            tail_max_permille: (u64::from(tail) * 1000).checked_div(n).unwrap_or(0) as u32,
            high,
        }
    }
}

/// The survey definition: every printed row, version and option of `sheet`, no key, the default
/// key-sheet rule (markers 0 and 9, found in the stack) and the sheet's ID scheme. An own sheet
/// takes its spec's questions, forms and cancel mode; a spec without a cancel mode is refused,
/// as defining the exam refuses it.
pub fn survey_definition(sheet: &Sheet, roster: Option<&Path>) -> Result<ExamDefinition, AppError> {
    let printed = define::printed(sheet)?;
    let mut i = Inputs::new(sheet.clone(), printed.questions);
    i.forms = Some(printed.versions);
    i.title = Some("Survey".into());
    let (mut d, _) = define::define(&i)?;
    // Every printed version is declared: nothing is held for its version.
    d.hold_undeclared_versions = false;
    if let Some(rows) = roster.and_then(|r| define::read_roster_path(r).ok()) {
        // A roster the exam would refuse is left out (defining the exam names its rows).
        let _ = d.import_roster(rows);
    }
    Ok(d)
}

fn file_sha384(p: &Path) -> String {
    match std::fs::read(p) {
        Ok(b) => omr_store::jcs::sha384_hex(&b),
        Err(e) => format!("unreadable: {e}"),
    }
}

/// The digest of a survey's request: the sheet (its template, or its spec file's bytes), every
/// scan and named key-sheet file with its size and modification time (sorted, so the input
/// order does not matter), and the roster's bytes. Cheap: the scans are not read.
pub fn request_digest(
    sheet: &Sheet,
    scans: &[PathBuf],
    key_sheets: &[PathBuf],
    roster: Option<&Path>,
) -> Result<String, AppError> {
    let files = |inputs: &[PathBuf]| -> Result<Vec<serde_json::Value>, AppError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let mut v: Vec<(String, u64, u128)> = crate::ingest::collect_paths(inputs)?
            .into_iter()
            .map(|p| {
                let m = std::fs::metadata(&p).ok();
                let modified = m
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                (p.display().to_string(), m.map_or(0, |m| m.len()), modified)
            })
            .collect();
        v.sort();
        v.dedup();
        Ok(v.into_iter()
            .map(|(p, len, t)| serde_json::json!([p, len, t.to_string()]))
            .collect())
    };
    let sheet = match sheet {
        Sheet::Template(t) => serde_json::json!({ "template": t }),
        Sheet::Spec(p) => {
            serde_json::json!({ "spec": p.display().to_string(), "sha384": file_sha384(p) })
        }
    };
    let body = serde_json::json!({
        "sheet": sheet,
        "scans": files(scans)?,
        "key_sheets": files(key_sheets)?,
        "roster": roster.map(file_sha384),
    });
    Ok(omr_store::jcs::sha384_hex(body.to_string().as_bytes()))
}

/// Surveys the scans of `req` ([`Survey`]): one recognition pass in memory, counts only.
pub fn survey(
    req: &SurveyRequest,
    progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Survey, AppError> {
    let def = survey_definition(&req.sheet, req.roster.as_deref())?;
    let request = request_digest(
        &req.sheet,
        &req.scans,
        &req.key_sheets,
        req.roster.as_deref(),
    )?;
    let opts = GradeOptions {
        threads: req.threads,
        project_dir: None,
        cancel: req.cancel.clone(),
        ..GradeOptions::default()
    };
    let g = pipeline::grade_with_key_sheets(&def, &req.scans, &req.key_sheets, &opts, progress)?;
    let roster = req.roster.as_deref().map(file_sha384);
    Ok(summarize(&g.session, request, roster))
}

fn marked(r: &Response) -> bool {
    matches!(
        r,
        Response::Single(_) | Response::Multi(_) | Response::Set(_)
    )
}

/// The counts of a surveyed session (the session itself is dropped by the caller).
fn summarize(s: &Session, request: String, roster: Option<String>) -> Survey {
    let def = &s.definition;
    let digits = match &def.template {
        TemplateChoice::Generated(spec) => usize::from(spec.id_digits),
        TemplateChoice::Legacy(_) => 10,
    };
    let schemes: Vec<(String, IdSchemeChoice)> = vec![
        ("iau".into(), IdSchemeChoice::Iau),
        ("pnu".into(), IdSchemeChoice::Pnu),
        ("ksu".into(), IdSchemeChoice::Ksu),
        (format!("digits:{digits}"), IdSchemeChoice::Digits(digits)),
    ];
    let schemes: Vec<(String, omr_identity::StudentIdScheme)> =
        schemes.into_iter().map(|(n, c)| (n, c.scheme())).collect();
    let options = usize::from(def.options);
    let mut rows = vec![
        RowUse {
            by_option: vec![0; options],
            ..RowUse::default()
        };
        usize::from(def.questions)
    ];
    let mut versions = VersionCounts::default();
    let mut last_rows: BTreeMap<u16, u32> = BTreeMap::new();
    let mut ids = IdCounts::default();
    let (mut booklets, mut blank_booklets, mut multi_marks) = (0, 0, 0);
    let mut exam_types: BTreeMap<String, u32> = BTreeMap::new();
    let mut sections: BTreeMap<String, u32> = BTreeMap::new();
    for b in &s.booklets {
        let Some(last) = b
            .questions
            .iter()
            .rposition(|q| marked(&q.recognized.response))
        else {
            blank_booklets += 1;
            continue;
        };
        booklets += 1;
        *last_rows.entry(last as u16 + 1).or_default() += 1;
        for (row, q) in rows.iter_mut().zip(&b.questions) {
            let r = &q.recognized.response;
            if marked(r) {
                row.marked += 1;
            }
            match r {
                Response::Single(o) => {
                    if let Some(c) = row.by_option.get_mut(usize::from(*o)) {
                        *c += 1;
                    }
                }
                Response::Multi(v) | Response::Set(v) => {
                    row.multi += 1;
                    for o in v {
                        if let Some(c) = row.by_option.get_mut(usize::from(*o)) {
                            *c += 1;
                        }
                    }
                    if q.recognized.auto_accepted {
                        multi_marks += 1;
                    }
                }
                _ => {}
            }
        }
        match b.declared_text.as_str() {
            "" => versions.blank += 1,
            "?" => versions.unclear += 1,
            l => *versions.by_letter.entry(l.to_string()).or_default() += 1,
        }
        let grid = b
            .pages
            .iter()
            .filter_map(|&i| s.pages[i].read.as_ref())
            .find_map(|r| r.student_id.as_ref());
        if let Some(g) = grid.filter(|g| g.confident()) {
            let text = g.text();
            if !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()) {
                ids.complete += 1;
                for (name, scheme) in &schemes {
                    if scheme.accepts(&text) {
                        *ids.fit.entry(name.clone()).or_default() += 1;
                    }
                }
            }
        }
        if !b.exam_type.is_empty() {
            *exam_types.entry(b.exam_type.clone()).or_default() += 1;
        }
        if !b.sheet_section.is_empty() {
            *sections.entry(b.sheet_section.clone()).or_default() += 1;
        }
    }
    let count = |st: PageStatus| s.pages.iter().filter(|p| p.status == st).count();
    let not_this_sheet = s
        .pages
        .iter()
        .filter(|p| p.status == PageStatus::Unattributed)
        .map(|p| PageNote {
            label: mask_numbers(&crate::keysheet::page_label(&s.sources, p)),
            code: p
                .read
                .as_ref()
                .and_then(|r| {
                    r.exceptions
                        .iter()
                        .find(|e| e.code.starts_with("REG") || e.code.starts_with("IDN"))
                })
                .map_or_else(|| "REG-001".to_string(), |e| e.code.clone()),
        })
        .collect();
    let photos = s
        .pages
        .iter()
        .filter(|p| p.status == PageStatus::Read && p.read.as_ref().is_some_and(|r| r.photo))
        .count();
    let markers_needed = !def.key_sheets.markers.is_empty();
    let key_sheets = s
        .key_sheets
        .sheets
        .iter()
        .map(|k| {
            let reads: Vec<_> = k.pages.iter().map(|&i| s.pages[i].read.as_ref()).collect();
            let readable = reads.iter().all(Option::is_some);
            let registered = reads.iter().all(|r| r.is_some_and(|r| r.registered));
            let marker_ok = k.marker.is_some() || !markers_needed;
            KeySheetSeen {
                label: mask_numbers(&k.label),
                named: k.declared,
                marker: k.marker,
                version: k.form_text.clone(),
                last_row: reads
                    .iter()
                    .flatten()
                    .flat_map(|r| &r.questions)
                    .filter(|q| marked(&q.response))
                    .map(|q| q.question + 1)
                    .max()
                    .unwrap_or(0),
                problem: k
                    .refusal
                    .as_deref()
                    .filter(|_| !(readable && registered && marker_ok))
                    .map(mask_numbers),
            }
        })
        .collect();
    let mut sources: Vec<&str> = s.sources.iter().map(|x| x.sha384.as_str()).collect();
    sources.sort_unstable();
    let named: BTreeSet<&String> = def.key_sheets.declared.iter().collect();
    let fingerprint = omr_store::jcs::sha384_hex(
        serde_json::json!({
            "sources": sources,
            "template": s.manifest.template_sha384,
            "key_sheets": named,
            "roster": roster,
        })
        .to_string()
        .as_bytes(),
    );
    Survey {
        fingerprint,
        request,
        template: def.template.id(),
        files: s.sources.len(),
        pages: s.pages.len(),
        booklets,
        blank_booklets,
        blank_pages: count(PageStatus::Blank),
        duplicates: count(PageStatus::Duplicate),
        refused: count(PageStatus::Refused),
        photos,
        not_this_sheet,
        versions,
        rows,
        last_rows,
        multi_marks,
        ids,
        key_sheets,
        key_sheet_notes: s.key_sheets.notes.iter().map(|n| mask_numbers(n)).collect(),
        exam_types,
        sections,
    }
}
