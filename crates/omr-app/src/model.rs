//! The in-memory exam state (the "session"): pages, booklets, the effective key and the review
//! queue. It is derived from the project store's originals and decisions and saved as a
//! snapshot next to the store, so the CLI and the GUI can reopen a graded exam.

use crate::engine::PageRead;
use crate::exam::{ExamDefinition, ParsedKey};
use crate::ingest::SourceKind;
use crate::review::ReviewState;
use omr_identity::IdentityStatus;
use omr_imaging::CaptureKind;
use omr_model::{EngineManifest, Exception, QuestionOutcome, Response, Severity};
use serde::{Deserialize, Serialize};

/// Snapshot schema version.
pub const SESSION_SCHEMA: u32 = 1;

/// One input file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// File name.
    pub name: String,
    /// SHA-384 of the bytes.
    pub sha384: String,
    /// BLAKE3 address in the project's object store.
    pub blake3: Option<String>,
    /// Kind.
    pub kind: SourceKind,
    /// Pages.
    pub pages: u32,
}

/// Where a page stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageStatus {
    /// Recognised and attached to a booklet.
    Read,
    /// Refused at capture (CAP-001, CAP-004): quarantined.
    Refused,
    /// Not registered or not this exam's sheet (REG-001, IDN-002): not attached.
    Unattributed,
    /// Byte-identical duplicate of an ingested file (CAP-011).
    Duplicate,
    /// A blank back or a page showing only bleed-through (CAP-012, INFO): dropped, never
    /// collated, attributed or queued (§7.5).
    Blank,
    /// A key sheet (§10.1): the instructor's filled answer sheet, named with `--key-sheet` or
    /// found in the stack by the key-sheet rule ([`crate::exam::KeySheetRule`]). Read by the
    /// same recognizer, routed to the key store ([`Session::key_sheets`]), never a student
    /// booklet and never in the students' batch statistics.
    KeySheet,
}

/// One page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    /// Content-derived page ID.
    pub page_id: String,
    /// Index into `Session::sources`.
    pub source: usize,
    /// 0-based page index within the source.
    pub index: u32,
    /// Status.
    pub status: PageStatus,
    /// Capture classification.
    pub capture: Option<CaptureKind>,
    /// Resolution of the original, when known.
    pub source_dpi: Option<u32>,
    /// Working image size (px).
    pub working: Option<(u32, u32)>,
    /// Decided page (batch stage).
    pub read: Option<PageRead>,
    /// The capture refusal, if refused.
    pub refusal: Option<Exception>,
    /// Booklet index.
    pub booklet: Option<usize>,
}

/// How a booklet's form was established (§10.3 priority order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormSource {
    /// MAC-verified page code.
    Code,
    /// Bubbled version grid, confident.
    Bubbled,
    /// Statistical inference (provisional, SCO-005 `inferred`).
    Inferred,
    /// Chosen by a reviewer.
    Reviewed,
    /// Unknown.
    Unknown,
}

/// Form inference evidence (§10.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormInference {
    /// Log-likelihood per form (nats, rounded to 1/1000).
    pub ll_milli: Vec<i64>,
    /// Argmax form.
    pub argmax: usize,
    /// LL(argmax) − LL(runner-up), milli-nats.
    pub margin_milli: i64,
}

/// Review status of one response (§10.1 L1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseStatus {
    /// Auto-accepted by recognition.
    Auto,
    /// Waiting for review (open exception).
    Pending,
    /// A reviewer confirmed the proposal.
    Confirmed,
    /// A reviewer changed it.
    Changed,
}

/// One question of a booklet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionState {
    /// Recognition output.
    pub recognized: QuestionOutcome,
    /// Current (reviewed) response.
    pub current: Response,
    /// Status.
    pub status: ResponseStatus,
    /// Page index holding the question.
    pub page: usize,
}

/// Identity of a booklet (§9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityState {
    /// Grid read (`?` uncertain, `_` blank), masked when needed.
    pub read: String,
    /// Suggested or accepted student.
    pub student: Option<String>,
    /// Decision status.
    pub status: IdentityStatus,
    /// Top candidates (student key, posterior).
    pub candidates: Vec<(String, f64)>,
    /// IDN exceptions.
    pub exceptions: Vec<Exception>,
    /// Confirmed by a reviewer.
    pub confirmed: bool,
}

/// One booklet (a student's sheet: one or more pages).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Booklet {
    /// Booklet ID.
    pub booklet_id: String,
    /// Serial (page code, or the canonical rank for generic sheets).
    pub serial: u32,
    /// Page indices (into `Session::pages`), template page order.
    pub pages: Vec<usize>,
    /// Declared form (code or confident bubble), index into the definition's forms.
    pub declared_form: Option<usize>,
    /// What the version grid shows (`A`, `` blank, `?` uncertain).
    pub declared_text: String,
    /// Form used for scoring.
    pub form: Option<usize>,
    /// How `form` was established.
    pub form_source: FormSource,
    /// Inference evidence.
    pub inference: Option<FormInference>,
    /// A booklet-level blocking code (MRK-015 blank booklet) that holds the whole score as
    /// pending until a reviewer acknowledges it (never auto-scored 0).
    #[serde(default)]
    pub hold: Option<String>,
    /// Questions `0..n` (exam order).
    pub questions: Vec<QuestionState>,
    /// Identity.
    pub identity: IdentityState,
    /// Booklet-level exceptions (MRK-015, IDN-003, SCO-005 …).
    pub exceptions: Vec<Exception>,
    /// The section bubbled on the sheet (IAU sheet; empty when not bubbled or not read
    /// confidently): the section of a rosterless exam, checked against the roster's
    /// otherwise (IDN-004 `cross_section`).
    #[serde(default)]
    pub sheet_section: String,
    /// The exam type bubbled on the sheet (IAU sheet), if confident.
    #[serde(default)]
    pub exam_type: String,
}

impl Booklet {
    /// The student the booklet is attributed to (accepted or suggested).
    pub fn student(&self) -> Option<&str> {
        self.identity.student.as_deref()
    }

    /// Display label, e.g. "Booklet 0007".
    pub fn label(&self) -> String {
        format!("Booklet {:04}", self.serial)
    }
}

/// The key in force: per form, per question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectiveKey {
    /// Key version number (1 = first).
    pub version: u32,
    /// True when inferred from the sheets (never releasable).
    pub inferred: bool,
    /// Entries per form, per question.
    pub forms: Vec<Vec<KeyCell>>,
    /// Points per question (exact decimal strings).
    pub points: Vec<String>,
    /// Master items shared across forms (identical forms) or one set per form.
    pub shared_items: bool,
}

/// One key cell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyCell {
    /// Accepted options.
    Options(Vec<u8>),
    /// Voided.
    Void,
    /// Missing.
    Missing,
}

impl From<ParsedKey> for KeyCell {
    fn from(p: ParsedKey) -> Self {
        match p {
            ParsedKey::Options(v) => KeyCell::Options(v),
            ParsedKey::Void => KeyCell::Void,
            ParsedKey::Missing => KeyCell::Missing,
        }
    }
}

impl KeyCell {
    /// Display text, e.g. "B", "A|C", "*", "".
    pub fn text(&self) -> String {
        match self {
            KeyCell::Options(v) => v
                .iter()
                .map(|o| char::from(b'A' + o).to_string())
                .collect::<Vec<_>>()
                .join("|"),
            KeyCell::Void => "*".into(),
            KeyCell::Missing => String::new(),
        }
    }
}

/// One question of a key sheet as read (never auto-accepted, §10.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySheetQuestion {
    /// Recognition output (key-blind, as for any sheet).
    pub recognized: QuestionOutcome,
    /// Page index (into `Session::pages`) holding the question.
    pub page: usize,
    /// What the row gives the key (`omr-scoring` routing).
    pub cell: omr_scoring::SheetCell,
}

/// A key sheet (§10.1): the instructor's filled answer sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySheet {
    /// Pages (indices into `Session::pages`), template page order.
    pub pages: Vec<usize>,
    /// Named with `--key-sheet` (else found in the stack by the rule).
    pub declared: bool,
    /// The student-number grid as read (`0000000000`; `?` uncertain, `_` blank).
    pub id_read: String,
    /// The marker digit the grid shows, if it shows one.
    pub marker: Option<u8>,
    /// Version (index into the definition's forms), when declared on the sheet and used by
    /// the exam.
    pub form: Option<usize>,
    /// What the version grid or page code shows (`A`, `` blank, `?` uncertain).
    pub form_text: String,
    /// Why the sheet cannot give a key (not readable, not registered, no marker, version not
    /// declared or not one of the exam's): what happened and what to do (R10). A refused key
    /// sheet holds the run until it is rescanned or the key comes from a file.
    pub refusal: Option<String>,
    /// Questions `0..n` (exam order); empty when refused before reading.
    pub questions: Vec<KeySheetQuestion>,
    /// Where the sheet is: its first page's file and page number (e.g. `scans.pdf page 31`).
    #[serde(default)]
    pub label: String,
}

impl KeySheet {
    /// True when the sheet can give its version's key (read, marker, version).
    pub fn usable(&self) -> bool {
        self.refusal.is_none() && self.form.is_some()
    }
}

/// The instructor's decision on one key-sheet row (`omr key confirm --answer 7=B`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRowDecision {
    /// Version (index into the definition's forms).
    pub form: usize,
    /// Question (0-based).
    pub question: usize,
    /// Entry: `B`, `A|C` (explicit multi-accept), `*` (void).
    pub answer: String,
}

/// The instructor's confirmation of one version's key-sheet key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyConfirmation {
    /// Version (index into the definition's forms).
    pub form: usize,
    /// Confirming principal.
    pub by: String,
    /// When (RFC 3339 UTC).
    pub at_utc: String,
    /// The key-sheet scans confirmed: SHA-384 of the source file and the 1-based page.
    pub scans: Vec<(String, u32)>,
}

/// Key sheets of an exam and their confirmation state (§10.1).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct KeySheets {
    /// Key sheets in canonical page order.
    pub sheets: Vec<KeySheet>,
    /// The instructor's decisions on rows, latest per (form, question).
    pub decisions: Vec<KeyRowDecision>,
    /// Confirmed versions.
    pub confirmed: Vec<KeyConfirmation>,
    /// True while the key in force comes (in part) from key sheets not yet confirmed: nothing
    /// is scored, the run is held (SCO-003 BLOCKER, `key_sheet_unconfirmed`).
    pub pending: bool,
    /// A key file replaced the key-sheet key (`omr key replace`).
    #[serde(default)]
    pub superseded: bool,
    /// Store rows of the hold, resolved at confirmation.
    #[serde(default)]
    pub hold_rows: Vec<i64>,
    /// Notes on the rule (markers not used in the stack under the ID scheme or roster).
    #[serde(default)]
    pub notes: Vec<String>,
}

impl KeySheets {
    /// True when the exam has key sheets.
    pub fn any(&self) -> bool {
        !self.sheets.is_empty()
    }

    /// True when version `form` is confirmed.
    pub fn is_confirmed(&self, form: usize) -> bool {
        self.confirmed.iter().any(|c| c.form == form)
    }

    /// Versions that have at least one usable key sheet, ascending.
    pub fn forms(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .sheets
            .iter()
            .filter(|k| k.usable())
            .filter_map(|k| k.form)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The instructor's decision on (form, question), if any.
    pub fn decision(&self, form: usize, question: usize) -> Option<&str> {
        self.decisions
            .iter()
            .rev()
            .find(|d| d.form == form && d.question == question)
            .map(|d| d.answer.as_str())
    }
}

/// Wall-clock timings of a grading run (ms).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Timings {
    /// Worker threads.
    pub threads: usize,
    /// Read + stage originals into the store.
    pub ingest_ms: f64,
    /// Decode + working resolution + page stage (parallel).
    pub page_stage_ms: f64,
    /// Baseline + decisions.
    pub batch_stage_ms: f64,
    /// Identity.
    pub identity_ms: f64,
    /// Form inference, key, score run.
    pub scoring_ms: f64,
    /// Item analysis + outcomes at the last export.
    pub analysis_ms: f64,
    /// Store writes after recognition.
    pub store_ms: f64,
    /// Reports and exports at the last export (analysis included).
    pub export_ms: f64,
    /// Total.
    pub total_ms: f64,
    /// Time from start to the first provisional page result.
    pub first_result_ms: f64,
    /// Pages processed.
    pub pages: usize,
}

/// The release of an exam's results (§10.8, §13.4): a deliberate act of the instructor, only
/// when nothing blocks it. The Blackboard upload is written only for released results.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// When (RFC 3339 UTC).
    pub at_utc: String,
    /// Who released (principal).
    pub by: String,
    /// The score run released.
    pub score_run: String,
    /// The key version in force.
    pub key_version: u32,
    /// SHA-384 (hex) of the released score run.
    pub results_sha384: String,
    /// What changed after the release (§10.8 post-release change), if anything: the results
    /// must then be released again.
    pub superseded: Option<String>,
}

/// The exam state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// Schema.
    pub schema: u32,
    /// The definition.
    pub definition: ExamDefinition,
    /// Snapshot time used in reports (RFC 3339 UTC).
    pub snapshot_utc: String,
    /// Engine manifest.
    pub manifest: EngineManifest,
    /// Inputs.
    pub sources: Vec<SourceInfo>,
    /// Pages in canonical order (source SHA-384, page index), key sheets (§10.1) after every
    /// other page so the students' page indices never depend on them. Persisted once, apart from the
    /// session (`pages.json`): pages never change after grading, the session does with every
    /// review action.
    #[serde(skip, default)]
    pub pages: Vec<PageState>,
    /// Booklets in canonical order. Persisted apart from the session header, in chunks
    /// (`booklets-NNNNN.json`), so a review action rewrites one chunk (a snapshot written
    /// before chunks holds them here and is still read).
    #[serde(default, skip_serializing)]
    pub booklets: Vec<Booklet>,
    /// Effective key.
    pub key: Option<EffectiveKey>,
    /// Batch-scope exceptions, de-duplicated (CAP-003, MRK-016).
    pub batch_exceptions: Vec<Exception>,
    /// Exam-scope exceptions (SCO-003 `inferred_key`, key and policy findings).
    pub exam_exceptions: Vec<Exception>,
    /// Review queue and history.
    pub review: ReviewState,
    /// Timings of the grading run.
    pub timings: Timings,
    /// The release of the results, if released.
    #[serde(default)]
    pub release: Option<Release>,
    /// Key sheets read with the exam and their confirmation (§10.1).
    #[serde(default)]
    pub key_sheets: KeySheets,
    /// Whether the mis-key flags (§12.2) of the current score run were judged and queued.
    #[serde(default)]
    pub flags: FlagGate,
}

/// The mis-key check of the release gate (§12.2, §8.1): every score run's probable mis-keys
/// (rank 1, SCO-001 ERROR) are judged with their flag stability and queued for the
/// instructor's acknowledgement before the results can be released. Until the check has run
/// for the current score run (a review decision or a key change makes a new one), the gate
/// reads "computing…" and blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagGate {
    /// The current score run (set at every save).
    #[serde(default)]
    pub current_run: Option<String>,
    /// The score run whose flags were last judged and queued (`ExamProject::judge_flags`).
    #[serde(default)]
    pub judged_run: Option<String>,
}

impl FlagGate {
    /// True while the current score run's flags have not been judged yet.
    pub fn computing(&self) -> bool {
        self.current_run.is_some() && self.current_run != self.judged_run
    }
}

impl Session {
    /// True when the results are released and nothing changed since.
    pub fn released(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|r| r.superseded.is_none())
    }

    /// Open ERROR/BLOCKER count across the queue.
    pub fn open_blocking(&self) -> usize {
        self.review
            .items
            .iter()
            .filter(|i| i.is_open() && i.severity >= Severity::Error)
            .count()
    }

    /// True while the key in force comes from key sheets the instructor has not confirmed:
    /// nothing is scored from it and release is refused (§10.1).
    pub fn key_unconfirmed(&self) -> bool {
        self.key_sheets.pending
    }

    /// The release gate: `None` when releasable, otherwise the reason.
    pub fn release_blocked(&self) -> Option<String> {
        let mut why = Vec::new();
        if self.key_unconfirmed() {
            why.push(
                "the key read from the key sheets is not confirmed (SCO-003 BLOCKER): check the \
                 key summary (key_sheet_summary.html, or omr key summary) and confirm it (omr key \
                 confirm); nothing is scored until then"
                    .to_string(),
            );
        }
        if self.key.as_ref().is_some_and(|k| k.inferred) {
            why.push("key INFERRED from the sheets (SCO-003): never releasable".to_string());
        }
        let open = self.open_blocking();
        if open > 0 {
            let miskeys = self
                .review
                .items
                .iter()
                .filter(|i| i.is_open() && i.code == "SCO-001")
                .count();
            if miskeys > 0 {
                why.push(format!(
                    "{open} ERROR/BLOCKER open, {miskeys} of them a probable mis-key (SCO-001): \
                     change the key, accept a second answer, void the item, or keep the key with \
                     a reason and a note (omr review list --lane scoring)"
                ));
            } else {
                why.push(format!("{open} ERROR/BLOCKER open"));
            }
        }
        if self.flags.computing() && !self.key_unconfirmed() {
            why.push(
                "the item analysis of the current results has not checked for a probable \
                 mis-key yet (§12.2 flag stability: computing…); it runs with the next export \
                 (omr export) or release"
                    .to_string(),
            );
        }
        match &self.key {
            None => why.push("no answer key".into()),
            // Rows still to decide are listed by the key summary and `omr key confirm`.
            Some(_) if self.key_unconfirmed() => {}
            // A key entry left empty refuses the score run (SCO-003, key incomplete): nothing
            // is releasable until every question has its key or is voided.
            Some(k) => {
                let missing: Vec<String> = k
                    .forms
                    .iter()
                    .enumerate()
                    .take(if k.shared_items { 1 } else { k.forms.len() })
                    .flat_map(|(f, cells)| {
                        let form = self.definition.forms.get(f).cloned().unwrap_or_default();
                        let shared = k.shared_items;
                        cells
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| **c == KeyCell::Missing)
                            .map(move |(q, _)| {
                                if shared {
                                    format!("Q{}", q + 1)
                                } else {
                                    format!("{form}-Q{}", q + 1)
                                }
                            })
                    })
                    .collect();
                if !missing.is_empty() {
                    why.push(format!(
                        "key incomplete (SCO-003): no key for {}; set each one (omr key set) or \
                         void it with *",
                        missing.join(", ")
                    ));
                }
            }
        }
        (!why.is_empty()).then(|| why.join("; "))
    }
}
