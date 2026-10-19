//! Key sheets as a key source (spec §10.1 "Sources"): the instructor's own filled answer sheet,
//! read by the same key-blind recognizer as the students' sheets, anywhere in the stack, and
//! routed here to the key store (`omr-scoring` turns each row into a key entry). Key-sheet
//! responses are **never** auto-accepted: the run is held (SCO-003 BLOCKER, sub-reason
//! `key_sheet_unconfirmed`) and nothing is scored until the instructor confirms the rendered
//! key summary (`ExamProject::confirm_key_sheets`, `omr key confirm`).
//!
//! - **Which sheets** ([`crate::exam::KeySheetRule`]): files named as key sheets, and in the
//!   stack a sheet whose every student-number column is bubbled with a marker digit (0 or 9 by
//!   default, numbers no IAU student can have); a key sheet must also declare its version on an
//!   exam of several versions, else it is refused with what to do (never a student). Key sheets are
//!   taken out of the student population before collation and out of the students' batch
//!   statistics, so the students read exactly as without them.
//! - **What they give**: per version, the rows read as one confident option; every other row
//!   (several marks, blank, an option the exam does not use, a mark not read confidently, two
//!   key sheets that disagree) waits for the instructor's decision and blocks confirmation.
//! - **Never a wrong key**: a named key sheet whose student-number grid does not read as a
//!   marker (a sheet registered one bubble row off reads `1111111111`) is refused, never read.
//! - **Views**: [`KeySheetSummary`] (per version, each question's read answer beside its
//!   rectified bubble-row crop, uncertain rows highlighted), [`summary_html`] and [`key_csv`].

use crate::AppError;
use crate::app::ExamProject;
use crate::crops::Crop;
use crate::engine::{GroupGeom, PageRead};
use crate::exam::{ExamDefinition, KeySource, parse_key_entry};
use crate::model::{
    EffectiveKey, KeyCell, KeySheet, KeySheetQuestion, KeySheets, PageState, PageStatus, Session,
    SourceInfo,
};
use crate::review::{OverlayBubble, OverlayState};
use omr_imaging::GrayImage;
use omr_model::exceptions::Target;
use omr_model::{Exception, QuestionOutcome, Response, Scope, Severity};
use omr_scoring::SheetCell;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Sub-reason of the hold (SCO-003 BLOCKER, Exam): the key comes from key sheets the
/// instructor has not confirmed yet. Appendix A's "key missing or incomplete … no key version
/// for a variant" fits: an unconfirmed key-sheet key is no published key version.
pub const HOLD: &str = "key_sheet_unconfirmed";

/// File name of the key summary written beside the exports.
pub const SUMMARY_FILE: &str = "key_sheet_summary.html";

/// The marker digit a page's student-number grid shows: every column a confident single digit
/// `d`, the same in all columns, and `d` one of `markers`. A number that merely starts with
/// zeros (`0000012345`) shows no marker.
pub fn grid_marker(read: &PageRead, markers: &[u8]) -> Option<u8> {
    let g = read.student_id.as_ref()?;
    if !g.confident() {
        return None;
    }
    let text = g.text();
    let first = *text.as_bytes().first()?;
    if !first.is_ascii_digit() || !text.bytes().all(|c| c == first) {
        return None;
    }
    let d = first - b'0';
    markers.contains(&d).then_some(d)
}

/// The version a page declares: a verified page code (by its printed letter), else a confident
/// version bubble. Returns the letter shown (empty when nothing is declared, `?` when the
/// bubble is not read confidently) and the exam's form index of that letter, if any.
pub fn declared_version(def: &ExamDefinition, read: &PageRead) -> (String, Option<usize>) {
    let text = if let Some(f) = read.code_form {
        f.letter().to_string()
    } else {
        match &read.form_choice {
            Some(c) if c.confident => c.value().unwrap_or_default().to_string(),
            Some(_) => "?".into(),
            None => String::new(),
        }
    };
    let form = def.forms.iter().position(|f| *f == text);
    (text, form)
}

/// True when a page in the students' stack is a key sheet under the rule: registered and its
/// grid shows a marker usable in the stack (`stack_markers`, numbers no student can have). The
/// version is not part of the rule: a marker sheet whose version is blank or unclear is still a
/// key sheet — on a one-version exam it is that version, otherwise it is refused with what to
/// do ([`collect`]) — and never graded as a student whose number is the marker.
pub fn is_stack_key_sheet(read: &PageRead, stack_markers: &[u8]) -> bool {
    read.registered && grid_marker(read, stack_markers).is_some()
}

/// The markers usable in the stack for this exam (scheme and roster checked), with notes on
/// the markers left out. `digits` is the number of student-number columns on the sheet.
pub fn stack_markers(def: &ExamDefinition, digits: usize) -> (Vec<u8>, Vec<String>) {
    def.key_sheets
        .stack_markers(digits, &def.id_scheme.scheme(), def.roster.as_deref())
}

fn page_label(sources: &[SourceInfo], p: &PageState) -> String {
    let name = sources
        .get(p.source)
        .map(|s| {
            s.name
                .split(" (unreadable: ")
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();
    format!("{name} page {}", p.index + 1)
}

/// The key sheets of a run, from the pages marked [`PageStatus::KeySheet`] (canonical order).
/// A sheet of several pages (own forms) is grouped by its verified serial, else by consecutive
/// pages of one file. Each row is routed by `omr-scoring` ([`SheetCell::route`]): only one
/// confidently read option gives a key entry, everything else waits for the instructor.
pub fn collect(
    def: &ExamDefinition,
    pages: &[PageState],
    sources: &[SourceInfo],
    declared: &[bool],
) -> Vec<KeySheet> {
    let n = usize::from(def.questions);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_serial: BTreeMap<(usize, u32), usize> = BTreeMap::new();
    for (i, p) in pages.iter().enumerate() {
        if p.status != PageStatus::KeySheet {
            continue;
        }
        let serial = p
            .read
            .as_ref()
            .and_then(|r| r.code_serial)
            .filter(|&s| s > 0);
        let page_no = p.read.as_ref().map_or(1, |r| r.page_no);
        if let Some(s) = serial {
            match by_serial.get(&(p.source, s)) {
                Some(&g) => groups[g].push(i),
                None => {
                    by_serial.insert((p.source, s), groups.len());
                    groups.push(vec![i]);
                }
            }
            continue;
        }
        let joins = page_no > 1
            && groups.last().is_some_and(|g| {
                let last = &pages[*g.last().expect("non-empty")];
                last.source == p.source && last.read.as_ref().is_some_and(|r| r.page_no < page_no)
            });
        if joins {
            groups.last_mut().expect("checked").push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
        .into_iter()
        .map(|mut g| {
            g.sort_by_key(|&i| pages[i].read.as_ref().map_or(0, |r| r.page_no));
            sheet_of(def, pages, sources, declared, g, n)
        })
        .collect()
}

fn sheet_of(
    def: &ExamDefinition,
    pages: &[PageState],
    sources: &[SourceInfo],
    declared: &[bool],
    g: Vec<usize>,
    n: usize,
) -> KeySheet {
    let first = &pages[g[0]];
    let label = page_label(sources, first);
    let is_declared = g.iter().any(|&i| declared.get(i).copied().unwrap_or(false));
    let markers = &def.key_sheets.markers;
    let head = g
        .iter()
        .filter_map(|&i| pages[i].read.as_ref())
        .find(|r| r.student_id.is_some());
    let id_read = head
        .and_then(|r| r.student_id.as_ref())
        .map(|g| g.text())
        .unwrap_or_default();
    let marker = head.and_then(|r| grid_marker(r, markers));
    let (form_text, mut form) = g
        .iter()
        .filter_map(|&i| pages[i].read.as_ref())
        .map(|r| declared_version(def, r))
        .find(|(t, _)| !t.is_empty())
        .unwrap_or_default();
    // One version only: an unbubbled version is that version.
    if form.is_none() && form_text.is_empty() && def.forms.len() == 1 {
        form = Some(0);
    }
    let refusal = if let Some(e) = first.refusal.as_ref().filter(|_| first.read.is_none()) {
        Some(format!(
            "{label} could not be read ({} {}); rescan the key sheet, or give the key as a file \
             (omr key replace --key key.csv)",
            e.code, e.detail
        ))
    } else if !g
        .iter()
        .all(|&i| pages[i].read.as_ref().is_some_and(|r| r.registered))
    {
        Some(format!(
            "{label} could not be registered as this exam's answer sheet, so its marks cannot be \
             trusted; rescan the key sheet flat (a scanner at 200–300 dpi, or its 600-dpi PDF), \
             or give the key as a file (omr key replace --key key.csv)"
        ))
    } else if !markers.is_empty() && marker.is_none() {
        let shown = markers
            .iter()
            .map(|d| {
                std::iter::repeat_n(char::from(b'0' + d), id_read.len().max(1)).collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" or ");
        Some(format!(
            "{label}: the student-number grid of a key sheet must be bubbled {shown}, but it reads \
             {}; the sheet may have been registered one bubble row off, or it is not a key \
             sheet. Not used as a key: bubble the marker on the key sheet and rescan it flat, or \
             give the key as a file (omr key replace --key key.csv)",
            if id_read.is_empty() {
                "nothing"
            } else {
                id_read.as_str()
            }
        ))
    } else if form.is_none() {
        Some(if form_text.is_empty() || form_text == "?" {
            format!(
                "{label}: the key sheet's version is not bubbled (or not read clearly); bubble \
                 the version (A, B …) on the key sheet and rescan it, or give the key as a file \
                 (omr key replace --key key.csv). If this page is a student's sheet bubbled with \
                 the marker, grade again naming the key sheets (--key-sheet FILE \
                 --key-sheet-marker none) so that it is graded as a student"
            )
        } else {
            format!(
                "{label}: the key sheet is for version {form_text}, which this exam does not \
                 use ({}); grade with --forms including {form_text}, or leave the sheet out",
                def.forms.join(", ")
            )
        })
    } else {
        None
    };
    let mut questions: Vec<Option<KeySheetQuestion>> = vec![None; n];
    if refusal.is_none() {
        for &pi in &g {
            let Some(r) = pages[pi].read.as_ref() else {
                continue;
            };
            for q in &r.questions {
                let qi = usize::from(q.question);
                if qi < n {
                    questions[qi] = Some(KeySheetQuestion {
                        cell: SheetCell::route(&q.response, q.auto_accepted, def.options),
                        recognized: q.clone(),
                        page: pi,
                    });
                }
            }
        }
    }
    let questions = if refusal.is_some() {
        Vec::new()
    } else {
        questions
            .into_iter()
            .enumerate()
            .map(|(q, s)| {
                s.unwrap_or_else(|| KeySheetQuestion {
                    recognized: QuestionOutcome {
                        question: q as u16,
                        response: Response::Unreadable { bubbles: vec![] },
                        auto_accepted: false,
                        logit: omr_model::LogitMilli(0),
                        exceptions: vec![
                            Exception::must("COL-001", Severity::Error, Scope::Booklet)
                                .with_target(Target {
                                    question: Some(q as u16),
                                    ..Target::default()
                                })
                                .with_detail("the key-sheet page holding this question is missing"),
                        ],
                    },
                    page: g[0],
                    cell: SheetCell::Uncertain { proposal: None },
                })
            })
            .collect()
    };
    KeySheet {
        pages: g,
        declared: is_declared,
        id_read,
        marker,
        form,
        form_text,
        refusal,
        questions,
        label,
    }
}

/// The rows of version `form`: the cells of its usable key sheets combined (rows on which two
/// sheets disagree are uncertain), or `None` when no usable sheet gives that version.
pub fn form_cells(ks: &KeySheets, form: usize) -> Option<Vec<SheetCell>> {
    let sheets: Vec<Vec<SheetCell>> = ks
        .sheets
        .iter()
        .filter(|k| k.usable() && k.form == Some(form))
        .map(|k| k.questions.iter().map(|q| q.cell.clone()).collect())
        .collect();
    (!sheets.is_empty()).then(|| omr_scoring::key_sheet::combine(&sheets))
}

/// True when the usable sheets of `form` disagree on question `q`.
fn sheets_disagree(ks: &KeySheets, form: usize, q: usize) -> bool {
    let mut cells = ks
        .sheets
        .iter()
        .filter(|k| k.usable() && k.form == Some(form))
        .filter_map(|k| k.questions.get(q).map(|x| &x.cell));
    let Some(first) = cells.next() else {
        return false;
    };
    cells.any(|c| c != first)
}

/// The key cells of version `form` from its key sheets and the instructor's decisions (a
/// decision wins; an undecided row that is not one confident option is missing, never
/// guessed). `None` when no usable sheet gives the version.
pub fn key_cells(def: &ExamDefinition, ks: &KeySheets, form: usize) -> Option<Vec<KeyCell>> {
    let cells = form_cells(ks, form)?;
    Some(
        cells
            .iter()
            .enumerate()
            .map(|(q, c)| match ks.decision(form, q) {
                Some(a) => parse_key_entry(a, def.options)
                    .map(KeyCell::from)
                    .unwrap_or(KeyCell::Missing),
                None => c
                    .key()
                    .map_or(KeyCell::Missing, |o| KeyCell::Options(vec![o])),
            })
            .collect(),
    )
}

/// Rows of version `form` still waiting for the instructor: (question 0-based, why).
pub fn open_rows(ks: &KeySheets, form: usize) -> Vec<(usize, String)> {
    let Some(cells) = form_cells(ks, form) else {
        return Vec::new();
    };
    cells
        .iter()
        .enumerate()
        .filter(|(q, c)| c.needs_decision() && ks.decision(form, *q).is_none())
        .map(|(q, c)| {
            let why = if sheets_disagree(ks, form, q) {
                "SCO-003 the key sheets of this version disagree: give the answer".to_string()
            } else {
                format!(
                    "{} {}",
                    c.code().unwrap_or_default(),
                    c.reason().unwrap_or_default()
                )
            };
            (q, why)
        })
        .collect()
}

/// Recomputes whether the key waits for the instructor: key sheets exist, no key file replaced
/// them, and some sheet is refused or its version is not confirmed.
pub fn refresh_pending(ks: &mut KeySheets) {
    ks.pending = !ks.superseded
        && ks.any()
        && !ks
            .sheets
            .iter()
            .all(|k| k.usable() && k.form.is_some_and(|f| ks.is_confirmed(f)));
}

/// Lays the key-sheet rows of every version that has key sheets and is not confirmed over
/// `key` (the key file's, the inferred one, or none): the key sheet is the instructor's key
/// for its version. Versions without a key sheet keep their source. When every version comes
/// from key sheets an inferred key is no longer inferred (its SCO-003 `inferred_key` notice
/// is dropped from `exam_exceptions`).
pub fn overlay(
    def: &ExamDefinition,
    ks: &KeySheets,
    key: &mut Option<EffectiveKey>,
    exam_exceptions: &mut Vec<Exception>,
) {
    if ks.superseded {
        return;
    }
    let forms: Vec<usize> = ks
        .forms()
        .into_iter()
        .filter(|f| !ks.is_confirmed(*f))
        .collect();
    if forms.is_empty() {
        return;
    }
    let n = usize::from(def.questions);
    let k = key.get_or_insert_with(|| {
        let (points, form_points) = EffectiveKey::points_from(def);
        EffectiveKey {
            version: 1,
            inferred: false,
            forms: vec![vec![KeyCell::Missing; n]; def.forms.len()],
            points,
            form_points,
            shared_items: def.forms.len() == 1,
        }
    });
    for f in forms {
        if let Some(cells) = key_cells(def, ks, f)
            && let Some(slot) = k.forms.get_mut(f)
        {
            *slot = cells;
        }
    }
    let covered = ks.forms();
    if k.inferred && (0..def.forms.len()).all(|f| covered.contains(&f)) {
        k.inferred = false;
        exam_exceptions
            .retain(|e| !(e.code == "SCO-003" && e.sub_reason.as_deref() == Some("inferred_key")));
    }
    if !k.inferred {
        k.shared_items = k.can_share_items(def);
    }
}

/// Versions given by more than one key sheet, at least one of them found in the stack by its
/// marker number (C4): every such sheet is named, agreeing or not, because a student who
/// bubbled the marker and answered like the key (a full-marks student) is read as a second key
/// sheet and would silently not be graded. The instructor checks that each is a key sheet, or
/// grades again naming the real one so the student is graded (R10).
pub fn stack_warnings(def: &ExamDefinition, ks: &KeySheets) -> Vec<String> {
    let mut out = Vec::new();
    for f in ks.forms() {
        let all: Vec<&KeySheet> = ks
            .sheets
            .iter()
            .filter(|k| k.usable() && k.form == Some(f))
            .collect();
        if all.len() < 2 || all.iter().all(|k| k.declared) {
            continue;
        }
        let names = all
            .iter()
            .map(|k| {
                format!(
                    "{} ({})",
                    if k.label.is_empty() {
                        "a page"
                    } else {
                        k.label.as_str()
                    },
                    if k.declared {
                        "named"
                    } else {
                        "found in the stack"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(format!(
            "version {} is given by {} key sheets: {names}. Check that each one is a key sheet: a \
             student who bubbled the marker number and answered like the key reads the same, and \
             would not be graded. If one is a student's sheet, grade again naming the real key \
             sheet (--key-sheet FILE --key-sheet-marker none) so that the student is graded",
            def.forms[f],
            all.len()
        ));
    }
    out
}

/// The hold (SCO-003 BLOCKER, Exam, `key_sheet_unconfirmed`) naming what waits for the
/// instructor and what to do (R10).
pub fn hold_exception(def: &ExamDefinition, ks: &KeySheets) -> Exception {
    let mut parts = Vec::new();
    for f in ks.forms() {
        if ks.is_confirmed(f) {
            continue;
        }
        let open = open_rows(ks, f).len();
        parts.push(if open == 0 {
            format!("version {} read, to confirm", def.forms[f])
        } else {
            format!(
                "version {} read, {open} question(s) need your decision",
                def.forms[f]
            )
        });
        let n = usize::from(def.questions);
        let disagree = (0..n).filter(|&q| sheets_disagree(ks, f, q)).count();
        if disagree > 0 {
            let sheets = ks
                .sheets
                .iter()
                .filter(|k| k.usable() && k.form == Some(f))
                .count();
            parts.push(format!(
                "the {sheets} key sheets of version {} disagree on {disagree} question(s): if one \
                 of them is a student's sheet bubbled with the marker, grade again naming the key \
                 sheet (--key-sheet FILE --key-sheet-marker none) so that the student is graded",
                def.forms[f]
            ));
        }
    }
    parts.extend(stack_warnings(def, ks));
    let refused = ks.sheets.iter().filter(|k| !k.usable()).count();
    if refused > 0 {
        parts.push(format!(
            "{refused} key sheet(s) not usable (see the key summary: rescan them, or give the \
             key as a file with omr key replace)"
        ));
    }
    Exception::must("SCO-003", Severity::Blocker, Scope::Exam)
        .with_sub_reason(HOLD)
        .with_target(Target::default())
        .with_detail(format!(
            "KEY SHEETS NOT CONFIRMED: {}. Key-sheet answers are never accepted automatically: \
             check every row of the key summary ({SUMMARY_FILE}, or omr key summary), decide the \
             highlighted rows, then confirm (omr key confirm). Nothing is scored until then",
            parts.join("; ")
        ))
}

/// Carries the instructor's decisions and confirmations of an earlier run of the exam over to
/// its re-run (an appended batch): a confirmation stands while the same scans are the key
/// sheets of that version; a new key sheet of the version waits for confirmation again.
pub(crate) fn carry(
    def: &ExamDefinition,
    prior: &Session,
    ks: &mut KeySheets,
    key: &mut Option<EffectiveKey>,
    exam_exceptions: &mut Vec<Exception>,
    sources: &[SourceInfo],
    pages: &[PageState],
) {
    let p = &prior.key_sheets;
    ks.decisions = p.decisions.clone();
    ks.superseded = p.superseded;
    let scans = |k: &KeySheet| -> Vec<(String, u32)> {
        k.pages
            .iter()
            .map(|&i| (sources[pages[i].source].sha384.clone(), pages[i].index + 1))
            .collect()
    };
    ks.confirmed = p
        .confirmed
        .iter()
        .filter(|c| {
            let now: Vec<(String, u32)> = ks
                .sheets
                .iter()
                .filter(|k| k.usable() && k.form == Some(c.form))
                .flat_map(scans)
                .collect();
            !now.is_empty() && now.iter().all(|s| c.scans.contains(s))
        })
        .cloned()
        .collect();
    refresh_pending(ks);
    if ks.pending {
        overlay(def, ks, key, exam_exceptions);
    }
}

/// The provenance of a key version confirmed from key sheets, for its `KeyVersionPublished`
/// event: source, every scan (SHA-384 of the file and the page) and the confirming operator.
pub fn provenance(session: &Session, operator: &str) -> Map<String, Value> {
    let ks = &session.key_sheets;
    let mut scans = Vec::new();
    for c in &ks.confirmed {
        for (sha, page) in &c.scans {
            scans.push(json!({
                "sha384": sha,
                "page": page,
                "form": session.definition.forms.get(c.form).cloned().unwrap_or_default(),
                "confirmed_by": c.by,
            }));
        }
    }
    let mut m = Map::new();
    m.insert("source".into(), json!("key_sheet"));
    m.insert("scans".into(), Value::Array(scans));
    m.insert("operator".into(), json!(operator));
    m.insert("decided_rows".into(), json!(ks.decisions.len() as u64));
    m
}

/// What [`ExamProject::confirm_key_sheets`] did.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyConfirmReport {
    /// Versions confirmed now.
    pub confirmed: Vec<String>,
    /// Versions read from key sheets still waiting for confirmation.
    pub waiting: Vec<String>,
    /// True while the run stays held (a version waits, or a key sheet cannot be read).
    pub held: bool,
    /// The key version published when the hold lifted.
    pub key_version: Option<u32>,
    /// Publish warnings on the confirmed key (SCO-027 letter runs or distribution).
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// The key summary the instructor confirms (§10.1): view-model, crops, HTML, CSV.

/// How a key-sheet row stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    /// One option read confidently (to be confirmed with the sheet).
    Read,
    /// Decided by the instructor.
    Decided,
    /// Several marks (SCO-027): needs a decision.
    MultiMark,
    /// No mark (SCO-003): needs a decision.
    Blank,
    /// An option the exam does not use (SCO-004): needs a decision.
    NotPrinted,
    /// Not read confidently (SCO-003): needs a decision.
    Uncertain,
    /// Two key sheets of the version disagree: needs a decision.
    Disagree,
}

impl RowStatus {
    /// Short label for displays.
    pub fn label(self) -> &'static str {
        match self {
            RowStatus::Read => "read",
            RowStatus::Decided => "decided by you",
            RowStatus::MultiMark => "several marks",
            RowStatus::Blank => "no mark",
            RowStatus::NotPrinted => "option not used",
            RowStatus::Uncertain => "not read clearly",
            RowStatus::Disagree => "key sheets disagree",
        }
    }

    /// True while the row waits for the instructor.
    pub fn open(self) -> bool {
        !matches!(self, RowStatus::Read | RowStatus::Decided)
    }
}

/// One row of a version's key summary.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyRowView {
    /// 1-based question.
    pub question: u32,
    /// What the sheet shows: `B`, `A+C`, `blank`, `B?` (an unconfident proposal), `?`.
    pub read: String,
    /// The key entry the row gives: `B`, `A|C`, `*`, or empty while undecided.
    pub answer: String,
    /// Status.
    pub status: RowStatus,
    /// Highlighted: waiting for the instructor, decided by them, an erasure the recognizer
    /// resolved (MRK-003), or a key file that gives another answer.
    pub highlight: bool,
    /// Why the row needs a decision and what to do, or the recognizer's codes.
    pub detail: String,
    /// The key file's entry when it differs from the key sheet (a key file was also given).
    pub key_file: Option<String>,
    /// Bubble fills δ (0..1), option order.
    pub fills: Vec<f32>,
    /// The rectified bubble row (RGBA with overlays), when the page image is available.
    pub crop: Option<Crop>,
}

/// One key sheet in the summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySheetInfo {
    /// File and page, e.g. `key-A.pdf page 1`.
    pub label: String,
    /// SHA-384 of the scanned file (hex).
    pub sha384: String,
    /// Named with `--key-sheet` (else found in the stack).
    pub declared: bool,
    /// Student-number grid as read.
    pub id_read: String,
    /// Version shown on the sheet.
    pub version_text: String,
    /// Registration summary, e.g. `matched 684/687, rms 0.41 px`.
    pub registration: String,
    /// Why it is not used, if so (R10).
    pub refusal: Option<String>,
}

/// One version's key from key sheets.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionKeyView {
    /// Version letter.
    pub version: String,
    /// The usable key sheets of the version.
    pub sheets: Vec<KeySheetInfo>,
    /// Rows, question order.
    pub rows: Vec<KeyRowView>,
    /// Rows waiting for the instructor.
    pub open: usize,
    /// Confirmed by whom and when, if confirmed.
    pub confirmed: Option<(String, String)>,
    /// True when the version can be confirmed now (no open row).
    pub can_confirm: bool,
}

/// The key summary of an exam's key sheets (shell view-model; [`summary_html`] renders it).
#[derive(Clone, Debug, PartialEq)]
pub struct KeySheetSummary {
    /// Exam title (course and title).
    pub exam: String,
    /// Versions read from key sheets, in the definition's order.
    pub versions: Vec<VersionKeyView>,
    /// Key sheets that cannot give a key (refused, version unknown).
    pub unusable: Vec<KeySheetInfo>,
    /// Exam versions without any key sheet, and where their key comes from.
    pub other_versions: Vec<(String, String)>,
    /// True while the run is held for the instructor's confirmation.
    pub pending: bool,
    /// A key file replaced the key sheets.
    pub superseded: bool,
    /// Rule notes (markers not used in the stack).
    pub notes: Vec<String>,
    /// The key's printed-letter checks (SCO-027 runs and letter distribution) with their full
    /// detail, letter counts included: for the owner-only summary page, never a terminal.
    pub key_checks: Vec<String>,
}

/// Text of a response as a key-sheet reading: `B`, `A+C`, `blank`, `?`.
fn read_text(r: &Response, labels: &[String]) -> String {
    let l = |o: &u8| {
        labels
            .get(usize::from(*o))
            .cloned()
            .unwrap_or_else(|| char::from(b'A' + o).to_string())
    };
    match r {
        Response::Single(o) => l(o),
        Response::Blank => "blank".into(),
        Response::Multi(v) | Response::Set(v) => v.iter().map(l).collect::<Vec<_>>().join("+"),
        Response::Unreadable { .. } => "?".into(),
    }
}

fn registration_text(r: &PageRead) -> String {
    let g = &r.registration;
    match g["engine"].as_str() {
        Some("legacy") => format!(
            "matched {}/{} bubbles, rms {:.2} px, scale {:.3}",
            g["matched"].as_i64().unwrap_or(0),
            g["template_points"].as_i64().unwrap_or(0),
            g["rms_px_milli"].as_i64().unwrap_or(0) as f64 / 1000.0,
            g["scale_milli"].as_i64().unwrap_or(0) as f64 / 1000.0
        ),
        Some("own") => format!(
            "{} tags, rms {} µm",
            g["tags"].as_i64().unwrap_or(0),
            g["rms_um"].as_i64().unwrap_or(0)
        ),
        _ => String::new(),
    }
}

fn sheet_info(s: &Session, k: &KeySheet) -> KeySheetInfo {
    let p = &s.pages[k.pages[0]];
    KeySheetInfo {
        label: page_label(&s.sources, p),
        sha384: s
            .sources
            .get(p.source)
            .map(|x| x.sha384.clone())
            .unwrap_or_default(),
        declared: k.declared,
        id_read: k.id_read.clone(),
        version_text: k.form_text.clone(),
        registration: p.read.as_ref().map(registration_text).unwrap_or_default(),
        refusal: k.refusal.clone(),
    }
}

/// How far a key-summary row crop reaches left of the row's first bubble, in option pitches:
/// far enough to show the question number printed beside the row (on the IAU sheet its box
/// sits ≈ 1.8 pitches left of option A), so a row read off by one row cannot look right under
/// its question (C5, §10.1).
pub const LABEL_LEAD_PITCHES: f32 = 2.3;

/// A bubble row resampled along its own axis (the registered geometry): straight and level
/// whatever the page's rotation or a photo's perspective, `scale` output px per working px.
/// `lead_pitches` option pitches before the first bubble are included (the printed question
/// number; 0 for the bubbles alone). Pixels outside the page are white; overlays are in crop
/// pixels.
pub fn rectified_row(
    img: &GrayImage,
    g: &GroupGeom,
    labels: &[String],
    marked: &[u8],
    scale: f32,
    lead_pitches: f32,
) -> Option<Crop> {
    let (first, last) = (g.bubbles.first()?, g.bubbles.last()?);
    let r = g.bubbles.iter().map(|b| b.r).fold(4.0f32, f32::max);
    let pad = r * 0.7;
    let (dx, dy) = (last.cx - first.cx, last.cy - first.cy);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = if len > 1e-3 {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let (vx, vy) = (-uy, ux);
    let half = r + pad;
    let pitch = if g.bubbles.len() > 1 {
        len / (g.bubbles.len() - 1) as f32
    } else {
        2.0 * half
    };
    let lead = half.max(lead_pitches * pitch);
    let (w, h) = (
        ((len + lead + half) * scale).ceil().max(1.0) as usize,
        ((2.0 * half) * scale).ceil().max(1.0) as usize,
    );
    if w > 4000 || h > 1000 {
        return None;
    }
    let (ox, oy) = (
        first.cx - ux * lead - vx * half,
        first.cy - uy * lead - vy * half,
    );
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            let px = ox + ux * sx + vx * sy;
            let py = oy + uy * sx + vy * sy;
            let v = omr_imaging::bilinear(img, f64::from(px), f64::from(py), 255);
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let bubbles = g
        .bubbles
        .iter()
        .map(|b| {
            let along = (b.cx - first.cx) * ux + (b.cy - first.cy) * uy;
            let across = (b.cx - first.cx) * vx + (b.cy - first.cy) * vy;
            OverlayBubble {
                label: labels
                    .get(usize::from(b.option))
                    .cloned()
                    .unwrap_or_else(|| b.option.to_string()),
                cx: (along + lead) * scale,
                cy: (across + half) * scale,
                r: b.r * scale,
                fill: b.fill,
                state: if b.unreadable {
                    OverlayState::Unreadable
                } else if marked.contains(&b.option) {
                    OverlayState::Filled
                } else if b.fill >= 0.15 {
                    OverlayState::Ambiguous
                } else {
                    OverlayState::Blank
                },
            }
        })
        .collect();
    Some(Crop {
        width: w as u32,
        height: h as u32,
        rgba: rgba.into(),
        bubbles,
    })
}

fn geom_of<'a>(s: &'a Session, q: &KeySheetQuestion) -> Option<&'a GroupGeom> {
    let r = s.pages.get(q.page)?.read.as_ref()?;
    let k = r
        .questions
        .iter()
        .position(|x| x.question == q.recognized.question)?;
    r.question_geom.get(k)
}

impl ExamProject {
    /// The key summary of the exam's key sheets (§10.1), `None` without key sheets. With
    /// `crops`, every row carries its rectified bubble-row crop from the stored original.
    pub fn key_sheet_summary(&self, crops: bool) -> Option<KeySheetSummary> {
        let s = &self.session;
        let ks = &s.key_sheets;
        if !ks.any() {
            return None;
        }
        let def = &s.definition;
        let labels = def.option_labels();
        let file_key: Option<Vec<crate::exam::FormKey>> = match &def.key {
            KeySource::Provided { forms } => Some(forms.clone()),
            _ => None,
        };
        let mut images: BTreeMap<usize, Option<std::sync::Arc<GrayImage>>> = BTreeMap::new();
        let mut versions = Vec::new();
        for f in ks.forms() {
            let sheets: Vec<&KeySheet> = ks
                .sheets
                .iter()
                .filter(|k| k.usable() && k.form == Some(f))
                .collect();
            let cells = form_cells(ks, f).unwrap_or_default();
            let entries = key_cells(def, ks, f).unwrap_or_default();
            let lead = sheets[0];
            let file_answers = file_key
                .as_ref()
                .and_then(|k| k.iter().find(|x| x.form == def.forms[f]))
                .map(|k| k.answers.clone());
            let mut rows = Vec::new();
            for (q, cell) in cells.iter().enumerate() {
                let kq = &lead.questions[q];
                let decided = ks.decision(f, q).is_some();
                let status = if decided {
                    RowStatus::Decided
                } else if sheets_disagree(ks, f, q) {
                    RowStatus::Disagree
                } else {
                    match cell {
                        SheetCell::Key { .. } => RowStatus::Read,
                        SheetCell::MultiMark { .. } => RowStatus::MultiMark,
                        SheetCell::Blank => RowStatus::Blank,
                        SheetCell::NotPrinted { .. } => RowStatus::NotPrinted,
                        SheetCell::Uncertain { .. } => RowStatus::Uncertain,
                    }
                };
                let mut read = read_text(&kq.recognized.response, &labels);
                if !kq.recognized.auto_accepted && read != "?" {
                    read.push('?');
                }
                let answer = entries.get(q).map(KeyCell::text).unwrap_or_default();
                // Also highlighted while the row reads one option: an erasure the recognizer
                // resolved (MRK-003), or a key file that says otherwise.
                let erased = kq.recognized.exceptions.iter().any(|e| e.code == "MRK-003");
                let detail = if status.open() {
                    open_rows(ks, f)
                        .into_iter()
                        .find(|(x, _)| *x == q)
                        .map(|(_, w)| w)
                        .unwrap_or_default()
                } else if decided {
                    "decided by you".into()
                } else if erased {
                    "MRK-003 an erased mark was resolved to this answer: check the crop".into()
                } else {
                    kq.recognized
                        .exceptions
                        .iter()
                        .map(|e| e.code.clone())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let key_file = file_answers.as_ref().and_then(|a| a.get(q)).and_then(|a| {
                    let fk = parse_key_entry(a, def.options)
                        .map(KeyCell::from)
                        .unwrap_or(KeyCell::Missing);
                    (entries.get(q) != Some(&fk)).then(|| fk.text())
                });
                let geom = geom_of(s, kq);
                let fills = geom
                    .map(|g| g.bubbles.iter().map(|b| b.fill).collect())
                    .unwrap_or_default();
                let crop = if crops {
                    let img = images
                        .entry(kq.page)
                        .or_insert_with(|| self.page_image(kq.page).ok())
                        .clone();
                    let marked: Vec<u8> = match &kq.recognized.response {
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
                rows.push(KeyRowView {
                    question: q as u32 + 1,
                    read,
                    answer,
                    highlight: status != RowStatus::Read || erased || key_file.is_some(),
                    status,
                    detail,
                    key_file,
                    fills,
                    crop,
                });
            }
            let open = rows.iter().filter(|r| r.status.open()).count();
            let confirmed = ks
                .confirmed
                .iter()
                .find(|c| c.form == f)
                .map(|c| (c.by.clone(), c.at_utc.clone()));
            versions.push(VersionKeyView {
                version: def.forms[f].clone(),
                sheets: sheets.iter().map(|k| sheet_info(s, k)).collect(),
                rows,
                open,
                can_confirm: open == 0 && confirmed.is_none(),
                confirmed,
            });
        }
        let covered = ks.forms();
        let other_versions = def
            .forms
            .iter()
            .enumerate()
            .filter(|(f, _)| !covered.contains(f))
            .map(|(_, v)| {
                let from = match &def.key {
                    KeySource::Provided { .. } => "the key file",
                    KeySource::Inferred => {
                        "INFERRED from the sheets (not releasable until replaced)"
                    }
                    KeySource::None => "no key yet (omr key set / omr key replace)",
                };
                (v.clone(), from.to_string())
            })
            .collect();
        Some(KeySheetSummary {
            exam: if def.course_code.is_empty() {
                def.title.clone()
            } else {
                format!("{} {}", def.course_code, def.title)
            },
            versions,
            unusable: ks
                .sheets
                .iter()
                .filter(|k| !k.usable())
                .map(|k| sheet_info(s, k))
                .collect(),
            other_versions,
            pending: ks.pending,
            superseded: ks.superseded,
            notes: ks
                .notes
                .iter()
                .cloned()
                .chain(stack_warnings(def, ks))
                .collect(),
            key_checks: self
                .scoring
                .as_ref()
                .map(|sc| {
                    sc.key
                        .letter_patterns(&sc.forms)
                        .iter()
                        .map(omr_scoring::LetterPattern::detail)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// The key-sheet key as a key file (`question,A,B,points`, the `--key` layout) for the
    /// versions read from usable key sheets; an undecided row is left empty (never guessed).
    /// `None` without usable key sheets.
    pub fn key_sheet_csv(&self) -> Option<String> {
        key_csv(&self.session)
    }
}

/// See [`ExamProject::key_sheet_csv`].
pub fn key_csv(s: &Session) -> Option<String> {
    let def = &s.definition;
    let ks = &s.key_sheets;
    let forms = ks.forms();
    if forms.is_empty() {
        return None;
    }
    let cells: Vec<Vec<KeyCell>> = forms
        .iter()
        .map(|&f| key_cells(def, ks, f).unwrap_or_default())
        .collect();
    let by_form = def.points_by_form();
    let per_form = !def.form_points.is_empty();
    let mut out = String::from("question");
    for &f in &forms {
        out.push(',');
        out.push_str(&def.forms[f]);
    }
    if per_form {
        for &f in &forms {
            out.push_str(",points_");
            out.push_str(&def.forms[f]);
        }
        out.push_str("\r\n");
    } else {
        out.push_str(",points\r\n");
    }
    for q in 0..usize::from(def.questions) {
        out.push_str(&(q + 1).to_string());
        for c in &cells {
            out.push(',');
            out.push_str(&c.get(q).map(KeyCell::text).unwrap_or_default());
        }
        if per_form {
            for &f in &forms {
                out.push(',');
                out.push_str(by_form[f].get(q).map_or("1", String::as_str));
            }
        } else {
            out.push(',');
            out.push_str(def.points.get(q).map_or("1", String::as_str));
        }
        out.push_str("\r\n");
    }
    Some(out)
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn crop_svg(c: &Crop) -> String {
    let png = crate::crops::png(c).unwrap_or_default();
    let data = data_encoding::BASE64.encode(&png);
    let mut svg = format!(
        "<svg class=\"crop\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" role=\"img\" \
         aria-label=\"bubble row\"><image href=\"data:image/png;base64,{data}\" width=\"{w}\" \
         height=\"{h}\"/>",
        w = c.width,
        h = c.height
    );
    for b in &c.bubbles {
        let class = match b.state {
            OverlayState::Filled => "f",
            OverlayState::Ambiguous => "a",
            OverlayState::Unreadable => "u",
            OverlayState::Blank => "b",
        };
        svg.push_str(&format!(
            "<circle class=\"{class}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\"/>",
            b.cx,
            b.cy,
            b.r + 1.5
        ));
    }
    svg.push_str("</svg>");
    svg
}

/// The key summary as a self-contained HTML page (crops embedded as PNG): per version, each
/// question's read answer beside its rectified bubble-row crop, every row that waits for the
/// instructor highlighted with what to do. `next` is the command line that confirms (shown in
/// the page).
pub fn summary_html(sum: &KeySheetSummary, next: &str) -> String {
    let mut h = String::new();
    h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    h.push_str(&format!("<title>Key summary · {}</title>", esc(&sum.exam)));
    h.push_str(
        "<style>:root{--bg:#fff;--fg:#1d2433;--mut:#5b6475;--line:#d9dde5;--hi:#fff4d6;\
         --hib:#b45309;--ok:#1a7f37;--bad:#b42318;--card:#f6f7f9}\
         @media (prefers-color-scheme:dark){:root{--bg:#14171c;--fg:#e6e9ef;--mut:#9aa3b2;\
         --line:#2c323c;--hi:#3a2f12;--hib:#f0b44c;--ok:#4cc270;--bad:#ff6b5b;--card:#1b1f26}}\
         body{background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,sans-serif;\
         margin:0;padding:16px;max-width:1100px}h1{font-size:22px;margin:0 0 4px}\
         h2{font-size:18px;margin:28px 0 8px}.mut{color:var(--mut)}\
         .banner{border:2px solid var(--hib);background:var(--hi);padding:10px 12px;\
         border-radius:6px;margin:12px 0}.ok{border-color:var(--ok);background:var(--card)}\
         .bad{border-color:var(--bad)}table{border-collapse:collapse;width:100%}\
         th,td{border-bottom:1px solid var(--line);padding:4px 8px;text-align:left;\
         vertical-align:middle}td.q{width:3em;font-variant-numeric:tabular-nums}\
         td.ans{font-weight:700;font-size:17px;width:5em}tr.hi td{background:var(--hi)}\
         tr.hi td.st{color:var(--hib);font-weight:600}.crop{max-width:100%;height:auto;\
         background:#fff;border:1px solid var(--line)}circle{fill:none;stroke-width:2}\
         circle.f{stroke:#1a7f37}circle.a{stroke:#d97706;stroke-dasharray:4 3}\
         circle.u{stroke:#b42318}circle.b{stroke:none}code{background:var(--card);\
         padding:1px 4px;border-radius:4px}.sheet{background:var(--card);padding:8px 10px;\
         border-radius:6px;margin:6px 0;font-size:13px;overflow-wrap:anywhere}\
         .wrap{overflow-x:auto}</style></head><body>",
    );
    h.push_str(&format!(
        "<h1>Answer key from key sheets</h1><p class=\"mut\">{} · exam-confidential: share \
         only with the exam's instructors</p>",
        esc(&sum.exam)
    ));
    let open: usize = sum.versions.iter().map(|v| v.open).sum();
    if sum.superseded {
        h.push_str(
            "<div class=\"banner ok\">A key file replaced the key sheets: this page is for \
             reference only.</div>",
        );
    } else if !sum.pending {
        h.push_str("<div class=\"banner ok\"><b>Confirmed.</b> The key below is in force.</div>");
    } else {
        h.push_str(&format!(
            "<div class=\"banner\" role=\"alert\"><b>Not confirmed yet: nothing is scored.</b> \
             Key-sheet answers are never accepted automatically. Check every row against the \
             crop. {}Then confirm: <code>{}</code></div>",
            if open > 0 {
                format!(
                    "{open} highlighted row(s) need your decision (add <code>--answer \
                     QUESTION=ANSWER</code>, e.g. <code>--answer 7=B</code>, <code>--answer \
                     7=A|C</code> to accept both, <code>--answer 7=*</code> to void). "
                )
            } else {
                String::new()
            },
            esc(next)
        ));
    }
    for k in &sum.unusable {
        h.push_str(&format!(
            "<div class=\"banner bad\" role=\"alert\"><b>Key sheet not used:</b> {}</div>",
            esc(k.refusal.as_deref().unwrap_or("version unknown"))
        ));
    }
    for n in &sum.notes {
        h.push_str(&format!("<p class=\"mut\">{}</p>", esc(n)));
    }
    for c in &sum.key_checks {
        h.push_str(&format!(
            "<p class=\"mut\">Key check (SCO-027, a warning): {}</p>",
            esc(c)
        ));
    }
    for v in &sum.versions {
        h.push_str(&format!(
            "<h2>Version {} — {} question(s), {}</h2>",
            esc(&v.version),
            v.rows.len(),
            match (&v.confirmed, v.open) {
                (Some((by, at)), _) => format!("confirmed by {} at {}", esc(by), esc(at)),
                (None, 0) => "every row read, to confirm".into(),
                (None, n) => format!("<b>{n} row(s) need your decision</b>"),
            }
        ));
        for k in &v.sheets {
            h.push_str(&format!(
                "<div class=\"sheet\">{} · {} · student-number grid <code>{}</code> · \
                 version {} · {} · SHA-384 <code>{}…</code></div>",
                esc(&k.label),
                if k.declared {
                    "named as a key sheet"
                } else {
                    "found in the stack"
                },
                esc(&k.id_read),
                esc(&k.version_text),
                esc(&k.registration),
                esc(k.sha384.get(..16).unwrap_or(&k.sha384))
            ));
        }
        h.push_str(
            "<p class=\"mut\">Each bubble row is shown as scanned, with the question number \
             printed beside it on the sheet: that number must match the Q column. If it does \
             not, the sheet was read a row off: do not confirm; rescan the key sheet flat.</p>\
             <div class=\"wrap\"><table><thead><tr><th>Q</th><th>Key</th><th>Bubble row as \
             scanned (with its printed number)</th><th>Read</th><th>Status</th></tr></thead>\
             <tbody>",
        );
        for r in &v.rows {
            h.push_str(&format!(
                "<tr{}><td class=\"q\">{}</td><td class=\"ans\">{}</td><td>{}</td><td>{}</td>\
                 <td class=\"st\">{}{}{}</td></tr>",
                if r.highlight { " class=\"hi\"" } else { "" },
                r.question,
                if r.answer.is_empty() {
                    "—".to_string()
                } else {
                    esc(&r.answer)
                },
                r.crop.as_ref().map(crop_svg).unwrap_or_default(),
                esc(&r.read),
                esc(r.status.label()),
                // A highlighted row always says why (an erased-and-changed answer reads like any
                // other row otherwise).
                if r.detail.is_empty() || (r.status == RowStatus::Read && !r.highlight) {
                    String::new()
                } else {
                    format!("<br><span class=\"mut\">{}</span>", esc(&r.detail))
                },
                r.key_file
                    .as_ref()
                    .map(|k| format!(
                        "<br><span class=\"mut\">the key file says {}</span>",
                        if k.is_empty() {
                            "nothing".into()
                        } else {
                            esc(k)
                        }
                    ))
                    .unwrap_or_default()
            ));
        }
        h.push_str("</tbody></table></div>");
    }
    if !sum.other_versions.is_empty() {
        h.push_str("<h2>Versions without a key sheet</h2><ul>");
        for (v, from) in &sum.other_versions {
            h.push_str(&format!("<li>Version {}: {}</li>", esc(v), esc(from)));
        }
        h.push_str("</ul>");
    }
    h.push_str("</body></html>\n");
    h
}

/// Reads key-sheet files on their own (`omr key read`): every sheet in `inputs` is a key
/// sheet, read in memory by the exam's recognizer (no project is written). The result answers
/// [`ExamProject::key_sheet_summary`] (crops included) and [`ExamProject::key_sheet_csv`].
pub fn read_key_sheets(
    def: &ExamDefinition,
    inputs: &[PathBuf],
    threads: usize,
) -> Result<ExamProject, AppError> {
    let paths = crate::ingest::collect_paths(inputs)?;
    let pool = crate::pipeline::pool(threads)?;
    use rayon::prelude::*;
    let sources: Vec<Result<crate::ingest::SourceFile, (PathBuf, String)>> = pool.install(|| {
        paths
            .par_iter()
            .map(|p| crate::ingest::SourceFile::read(p).map_err(|e| (p.clone(), e)))
            .collect()
    });
    let mut d = def.clone();
    d.key = KeySource::None;
    d.key_sheets.declared = sources
        .iter()
        .filter_map(|s| s.as_ref().ok().map(|f| f.sha384.clone()))
        .collect();
    let keep: Vec<crate::ingest::SourceFile> = sources
        .iter()
        .filter_map(|s| s.as_ref().ok().cloned())
        .collect();
    let opts = crate::pipeline::GradeOptions {
        threads,
        project_dir: None,
        snapshot_utc: Some(crate::pipeline::now_utc()),
        operator: "principal:operator".into(),
        max_pages: None,
    };
    let g = crate::pipeline::grade_sources(
        &d,
        sources,
        &opts,
        &pool,
        std::time::Instant::now(),
        &|_| {},
    )?;
    let p = ExamProject::from_graded(g);
    p.prefill_sources(keep);
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::BubbleGeom;

    #[test]
    fn a_rectified_row_is_level_whatever_the_rotation() {
        // A dark disk at each bubble centre along a row tilted by 20°.
        let (a, r): (f32, f32) = (0.35, 6.0);
        let centres: Vec<(f32, f32)> = (0..5)
            .map(|k| {
                (
                    40.0 + 25.0 * k as f32 * a.cos(),
                    50.0 + 25.0 * k as f32 * a.sin(),
                )
            })
            .collect();
        let img = GrayImage::from_fn(200, 140, |x, y| {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            if centres
                .iter()
                .any(|(cx, cy)| (px - cx).powi(2) + (py - cy).powi(2) <= r * r)
            {
                0
            } else {
                255
            }
        });
        let g = GroupGeom {
            bubbles: centres
                .iter()
                .enumerate()
                .map(|(k, (cx, cy))| BubbleGeom {
                    option: k as u8,
                    cx: *cx,
                    cy: *cy,
                    r,
                    fill: 0.9,
                    unreadable: false,
                })
                .collect(),
        };
        let labels: Vec<String> = ["A", "B", "C", "D", "E"].map(String::from).to_vec();
        let c = rectified_row(&img, &g, &labels, &[1], 1.0, 0.0).expect("crop");
        // Every overlay sits on one horizontal line, and each centre pixel is dark.
        let y0 = c.bubbles[0].cy;
        for b in &c.bubbles {
            assert!((b.cy - y0).abs() < 0.01);
            let (x, y) = (b.cx as usize, b.cy as usize);
            assert!(c.rgba[(y * c.width as usize + x) * 4] < 60, "{b:?}");
        }
        assert_eq!(c.bubbles[1].state, OverlayState::Filled);
        assert!(c.width as f32 > 100.0 && (c.height as f32) < 25.0);
    }
}
