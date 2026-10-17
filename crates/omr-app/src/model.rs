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
    /// Pages in canonical order (source SHA-384, page index). Persisted once, apart from the
    /// session (`pages.json`): pages never change after grading, the session does with every
    /// review action.
    #[serde(skip, default)]
    pub pages: Vec<PageState>,
    /// Booklets in canonical order.
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
}

impl Session {
    /// Open ERROR/BLOCKER count across the queue.
    pub fn open_blocking(&self) -> usize {
        self.review
            .items
            .iter()
            .filter(|i| i.is_open() && i.severity >= Severity::Error)
            .count()
    }

    /// The release gate: `None` when releasable, otherwise the reason.
    pub fn release_blocked(&self) -> Option<String> {
        let mut why = Vec::new();
        if self.key.as_ref().is_some_and(|k| k.inferred) {
            why.push("key INFERRED from the sheets (SCO-003): never releasable".to_string());
        }
        let open = self.open_blocking();
        if open > 0 {
            why.push(format!("{open} ERROR/BLOCKER open"));
        }
        if self.key.is_none() {
            why.push("no answer key".into());
        }
        (!why.is_empty()).then(|| why.join("; "))
    }
}
