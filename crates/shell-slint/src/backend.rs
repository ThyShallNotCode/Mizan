//! The view-model boundary between the Slint shell and the application core (spec §4.7, ADR-001).
//!
//! The shell renders and binds; it decides nothing. Everything it shows comes from a [`Backend`]
//! as the plain-Rust view-models below (no Slint types), and every user decision goes back as a
//! typed call. [`crate::app_backend::AppBackend`] implements [`Backend`] over `omr-app` (wrapped
//! in this crate, so the orphan rule holds); [`crate::demo::DemoBackend`] serves synthetic data.
//!
//! Conventions:
//! - Fractions are `0.0..=1.0` (`f32` for pixels and fills, `f64` for statistics); percentages
//!   are never pre-multiplied unless the field name says `_pct`.
//! - Display strings that the core owns (term labels, Hijri dates, TP-154 cells, flag messages)
//!   arrive formatted: the shell does not compute Hijri dates or report wording (§20.2, §11.7).
//! - Arabic strings arrive in logical order; the shell sets the Arabic font and lets the text
//!   engine shape them (§20.2).
//! - Lists are already in display order (queue order is owned by the core, §13.2).
//! - Key-isolation (§13.2, §19.3): review view-models never carry a key-derived value.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------------------------------ ids

/// Stable exam (assessment) identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExamId(pub String);

impl fmt::Display for ExamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable review-item (exception instance) identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "item-{}", self.0)
    }
}

/// Error returned by commands. The message is shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The exam, item or cell does not exist (any more).
    NotFound(String),
    /// The input was rejected by validation (e.g. a weight outside 0..=1).
    Invalid(String),
    /// The action is not allowed in the current state or for the current role.
    Denied(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m) | Self::Invalid(m) | Self::Denied(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for BackendError {}

// ----------------------------------------------------------------------------------- shared enums

/// Appendix A severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Error,
    Blocker,
}

impl Severity {
    /// Upper-case word used in counters and accessible labels.
    pub fn word(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Blocker => "BLOCKER",
        }
    }
}

/// Open exceptions by severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    pub info: u32,
    pub warn: u32,
    pub error: u32,
    pub blocker: u32,
}

impl SeverityCounts {
    pub fn total(&self) -> u32 {
        self.info + self.warn + self.error + self.blocker
    }
    /// ERROR + BLOCKER: the counts that hold release (§13.4).
    pub fn blocking(&self) -> u32 {
        self.error + self.blocker
    }
}

/// Exam lifecycle status as shown on the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExamStatus {
    Setup,
    Scanning,
    InReview,
    Ready,
    Released,
}

impl ExamStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Scanning => "Scanning",
            Self::InReview => "In review",
            Self::Ready => "Ready",
            Self::Released => "Released",
        }
    }
}

/// Generic tone for a status message, readiness check or table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Primary,
    Accent,
    Success,
    Warning,
    Error,
    Info,
}

// ------------------------------------------------------------------------------------- exams

/// Term label as the university writes it (§20.2: labels are never computed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TermLabel {
    /// e.g. "1448 H, 1st term".
    pub label: String,
    /// e.g. "الفصل الأول ١٤٤٨ هـ".
    pub label_ar: String,
    /// e.g. "Fall 2026".
    pub gregorian: String,
}

/// One row of the exams dashboard, plus what the detail drawer needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ExamSummary {
    pub id: ExamId,
    /// e.g. "CS 211".
    pub course_code: String,
    /// e.g. "Data Structures".
    pub course_title: String,
    /// Arabic course title (shaped by the shell, never mirrored).
    pub course_title_ar: String,
    /// e.g. "Midterm 1".
    pub assessment: String,
    pub term: TermLabel,
    /// Gregorian exam date, ISO "2026-10-12".
    pub date: String,
    /// Display-only Umm al-Qura equivalent, e.g. "1448/04/20 AH".
    pub date_hijri: String,
    /// Version (form) letters, e.g. ["A", "B"].
    pub versions: Vec<String>,
    pub status: ExamStatus,
    pub sheets_scanned: u32,
    pub sheets_expected: u32,
    pub open_exceptions: SeverityCounts,
    /// e.g. "Key v2 · published".
    pub key_status: String,
    /// Items with at least one approved CLO mapping, of `total_items` (every mapped item when
    /// `mapping_approval` is false).
    pub mapped_items: u32,
    /// True when the core approves mappings (§11.2), so `mapped_items` counts approved ones;
    /// false when every mapping of the definition is in force without an approval step.
    pub mapping_approval: bool,
    pub total_items: u32,
    pub roster_count: u32,
    pub sections: Vec<String>,
    /// e.g. "Today, 10:42".
    pub last_scan: String,
    /// e.g. "scn_8f291a".
    pub batch_id: String,
}

/// Dashboard stat tiles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DashboardStats {
    pub sheets_this_week: u32,
    pub sheets_today: u32,
    pub open_exceptions: u32,
    /// ERROR + BLOCKER among `open_exceptions`.
    pub open_errors: u32,
    /// `None` when review times are not recorded.
    pub median_review_secs: Option<f64>,
    /// `None` before any grading run.
    pub throughput_pages_per_sec: Option<f64>,
}

/// The signed-in user, for the top bar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserInfo {
    pub name: String,
    pub role: String,
    pub initials: String,
}

/// Exams dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub user: UserInfo,
    pub term: TermLabel,
    pub stats: DashboardStats,
    pub exams: Vec<ExamSummary>,
    /// Engine and manifest line for the status bar, e.g. "engine omr 0.1.0 · manifest 7f3a…b21c".
    pub engine: String,
    /// Workspace path or name for the status bar.
    pub workspace: String,
    /// Where the data comes from and what stays local (Settings · About).
    pub data_note: String,
}

// ------------------------------------------------------------------------------------- setup

/// Kind of answer-sheet template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// A third-party or legacy form described by a bubble lattice (§6.10), e.g. `iau-ccsit-v1`.
    Legacy,
    /// A sheet generated from the template model (§6.1) with fiducials and a page code.
    Generated,
}

/// Option label set of the template (§6.5, §20.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSet {
    Latin,
    Arabic,
    Numeric,
}

/// How students cancel a mark (§6.5 `cancel_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    EraseOnly,
    CrossCancels,
    FillCancelsCheck,
}

impl CancelMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::EraseOnly => "erase_only",
            Self::CrossCancels => "cross_cancels",
            Self::FillCancelsCheck => "fill_cancels_check",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateInfo {
    /// e.g. "iau-ccsit-v1" or "gen:a4-mcq-40x4 rev 3".
    pub id: String,
    pub kind: TemplateKind,
    pub name: String,
    /// Questions used by this exam.
    pub questions: u32,
    /// Questions printed on the sheet.
    pub capacity: u32,
    /// Printed option labels in order, e.g. ["A", "B", "C", "D", "E"].
    pub option_labels: Vec<String>,
    pub label_set: LabelSet,
    pub versions: Vec<String>,
    pub cancel_mode: CancelMode,
    pub dpi: u32,
    /// How pages are identified, e.g. "QR v3-M page code + AprilTag 36h11".
    pub page_identity: String,
    /// Template digest for provenance, abbreviated, e.g. "sha384 41c2…9e0d".
    pub digest: String,
}

/// Answer key of one version: one entry per question, as option letters
/// ("B"; "A,C" for a multi-key; "" unset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionKey {
    pub version: String,
    pub answers: Vec<String>,
}

/// NQF domain of a learning outcome (NCAAA): Knowledge and understanding, Skills, Values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NqfDomain {
    Knowledge,
    Skills,
    Values,
}

impl NqfDomain {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Knowledge => "K",
            Self::Skills => "S",
            Self::Values => "V",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Knowledge => "Knowledge and understanding",
            Self::Skills => "Skills",
            Self::Values => "Values, autonomy, and responsibility",
        }
    }
    pub fn title_ar(self) -> &'static str {
        match self {
            Self::Knowledge => "المعرفة والفهم",
            Self::Skills => "المهارات",
            Self::Values => "القيم والاستقلالية والمسؤولية",
        }
    }
}

/// A course learning outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloDef {
    /// e.g. "CLO1"; TP-154 numbering ("1.1") comes with the B.1 rows.
    pub code: String,
    pub title: String,
    pub title_ar: String,
    pub domain: NqfDomain,
    /// Related PLO code(s), e.g. "K1".
    pub plo: String,
}

/// One cell of the item × CLO matrix.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MappingCell {
    /// `None` = no mapping; otherwise `0 < w <= 1`.
    pub weight: Option<f64>,
    /// Free bilingual note (may contain Arabic).
    pub note: String,
}

/// Approval state of a mapping row (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingStatus {
    Draft,
    Proposed,
    Approved,
    /// Mapped and in force in a core without an approval step (never shown as "approved").
    InUse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MappingRow {
    /// 1-based master item number.
    pub question: u32,
    /// Points of the item.
    pub points: f64,
    /// One cell per CLO, in `ExamSetup::clos` order.
    pub cells: Vec<MappingCell>,
    pub status: MappingStatus,
}

/// How weights combine on an item (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingMode {
    /// Each mapped CLO receives the full item evidence (weights are independent).
    Full,
    /// The item's evidence is split: weights of a row must sum to 1.
    Split,
}

/// A setup lint (template, key or mapping), e.g. SCO-014 thin evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLint {
    pub code: String,
    pub severity: Severity,
    pub message: String,
}

/// Exam setup: template, keys, CLOs and the item × CLO mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct ExamSetup {
    pub exam: ExamId,
    pub template: TemplateInfo,
    pub keys: Vec<VersionKey>,
    /// e.g. "Key v2 · draft".
    pub key_status: String,
    pub key_published: bool,
    pub clos: Vec<CloDef>,
    pub mapping_mode: MappingMode,
    pub mapping: Vec<MappingRow>,
    pub lints: Vec<SetupLint>,
    /// Students on the roster; 0 when the exam has none (identities are then never matched).
    pub roster_students: u32,
}

// ------------------------------------------------------------------------------------- scan

/// State of one sheet thumbnail in the live batch view (DESIGN §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbState {
    /// Auto-accepted.
    Auto,
    /// Needs review: worst open code and its severity.
    Review { code: String, severity: Severity },
    /// Rejected at capture (e.g. CAP-002 below 100 dpi).
    Rejected { code: String, severity: Severity },
    /// The instructor's key sheet (§10.1), never a student: its version, and whether its key
    /// is confirmed (`refused`: it gives no key, e.g. its grid shows no key-sheet marker).
    KeySheet {
        version: String,
        confirmed: bool,
        refused: bool,
    },
    /// Being recognised now.
    Reading,
    /// Waiting in the spool.
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Booklet serial or imprint number, e.g. "0412".
    pub serial: String,
    pub state: ThumbState,
    /// The page as scanned, scaled down; `None` while the page is not available (the shell
    /// shows a blank sheet, never an invented one).
    pub image: Option<CropImage>,
}

/// Pipeline stage counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageCount {
    pub stage: String,
    pub count: u32,
}

/// Open exceptions of one code in the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeCount {
    pub code: String,
    pub title: String,
    pub severity: Severity,
    pub count: u32,
}

/// One key-value line of the batch reconciliation card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconLine {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

/// Sheets of the batch by state (the pills over the thumbnail grid): every sheet is counted
/// once, by its worst open item, whatever the number of items on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SheetCounts {
    pub auto: u32,
    pub review: u32,
    pub rejected: u32,
    /// The instructor's key sheets (never counted as students).
    pub key_sheets: u32,
}

/// Whether more scans can be added to a graded exam from the Scan screen (§8.1 later batch).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppendInfo {
    pub allowed: bool,
    /// What adding does, or why it cannot be done now (shown under the path field).
    pub note: String,
}

/// Live batch progress (DESIGN §9.3). A snapshot: the shell re-reads it after [`Backend::pump`].
#[derive(Debug, Clone, PartialEq)]
pub struct BatchProgress {
    pub exam: ExamId,
    /// e.g. "Batch 3 of 5".
    pub batch_label: String,
    /// e.g. "hot folder".
    pub source: String,
    /// e.g. "fi-8170 · 300 dpi · gray".
    pub device: String,
    pub running: bool,
    pub pages_done: u32,
    pub pages_total: u32,
    pub pages_per_sec: f64,
    /// Spec floor for the gauge tick (40 pages/s, §2.4).
    pub floor_pages_per_sec: f64,
    /// Gauge maximum (stretch target × 1.2).
    pub max_pages_per_sec: f64,
    /// Share of questions auto-accepted, 0..1.
    pub auto_accepted: f64,
    /// Abstention rate AR, 0..1.
    pub abstention_rate: f64,
    pub first_result: Duration,
    pub remaining: Duration,
    /// Pipeline: pages done, in flight and queued (sum = `pages_total`).
    pub in_flight: u32,
    pub queued: u32,
    pub stages: Vec<StageCount>,
    /// Sorted by severity, then count.
    pub exceptions: Vec<CodeCount>,
    /// Newest first.
    pub thumbnails: Vec<Thumbnail>,
    pub reconciliation: Vec<ReconLine>,
    /// Status of the reconciliation, e.g. Some(("COL-005", Error)).
    pub reconciliation_code: Option<(String, Severity)>,
    /// Newest first, mono lines.
    pub activity: Vec<String>,
    /// Engine manifest and workers for the status bar.
    pub engine: String,
    pub workers: u32,
    /// Sheets by state, for the pills over the thumbnails.
    pub sheets: SheetCounts,
    /// True when a capture agent runs the batch, so it can be paused and stopped.
    pub pausable: bool,
    /// Adding a later batch of scans to this exam.
    pub append: AppendInfo,
}

// ------------------------------------------------------------------------------------- review

/// Review lane (§13.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    Marks,
    Identity,
    Capture,
    Scoring,
}

impl Lane {
    pub const ALL: [Lane; 4] = [Lane::Marks, Lane::Identity, Lane::Capture, Lane::Scoring];
    pub fn label(self) -> &'static str {
        match self {
            Self::Marks => "Marks",
            Self::Identity => "Identity",
            Self::Capture => "Capture",
            Self::Scoring => "Scoring",
        }
    }
}

/// Open count of a lane the current role may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneCount {
    pub lane: Lane,
    pub open: u32,
}

/// Reviewer-facing bubble state drawn over the scan (DESIGN §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkState {
    /// Filled / selected: solid ring + check.
    Filled,
    /// Inside the MRK-001 band or a multi-candidate: dashed ring + "?".
    Ambiguous,
    /// UNREADABLE (never EMPTY): hatched square + "!".
    Unreadable,
    /// Blank: dotted ring + "–" (drawn only when overlays show all bubbles).
    Blank,
}

/// A reviewer's decision on an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    // ---- mark context: "What is marked?"
    /// One option, 0-based position (A = 0). Positional, whatever the label set (§20.2).
    Option(u8),
    /// Nothing is marked.
    Blank,
    /// More than one mark (invalid response).
    Multi,
    /// Cannot be read: rescan needed (never EMPTY).
    Unreadable,
    // ---- identity context: "Who is this?"
    /// The n-th candidate (0-based) of [`IdentityEvidence::candidates`].
    Candidate(u8),
    /// Not on the roster (`N`).
    NotOnRoster,
    /// Duplicate: two booklets resolve to one student (`D`).
    Duplicate,
    // ---- notice context (capture, scoring)
    /// The reviewer has seen the page- or exam-level exception and accepts the core's handling.
    Acknowledge,
}

/// Where a suggestion comes from (§13.3: shown with its source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionSource {
    Engine,
    Rule(String),
    Ai { model: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub decision: Decision,
    pub source: SuggestionSource,
    /// Fill of the suggested bubble, 0..1.
    pub fill: f32,
    /// Margin to the runner-up, 0..1 (for a [`ReviewKind::Form`] item: the log-likelihood margin
    /// of the inferred form in nats).
    pub margin: f32,
}

/// One bubble of the crop: geometry in crop pixels, plus the engine's reading.
#[derive(Debug, Clone, PartialEq)]
pub struct BubbleView {
    /// Printed label, e.g. "B" or "ب".
    pub label: String,
    /// Centre and radius in crop pixels (registered view).
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    /// Normalised darkness D (§8.5), 0..1.
    pub fill: f32,
    pub state: MarkState,
}

/// An RGBA8 image (straight alpha), row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CropImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// What kind of review frame an item needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    /// Ordinary mark exception: suggestion, fill bars and overlays shown.
    Mark,
    /// Blind verify sample (§13.4): no suggestion, fill bars, threshold or overlays.
    BlindVerify,
    /// Identity exception (§13.3): ID-grid crop, top candidates with per-channel evidence.
    Identity,
    /// Form (version) exception (§10.3): the version bubbles as options, the form letters as
    /// labels; only an option decides it (no Blank, Multi or Unreadable).
    Form,
    /// Page- or exam-level exception (capture, scoring): evidence lines and acknowledge.
    Notice,
}

/// One recognition channel's vote on a candidate (agree/disagree by colour *and* shape, §13.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelEvidence {
    /// e.g. "ID grid", "ID boxes", "Name", "Seat map".
    pub channel: String,
    pub agrees: bool,
    /// e.g. "9261 0344 17 (2 digits differ)".
    pub detail: String,
}

/// A roster candidate for an identity item. Synthetic in the demo; national IDs arrive masked.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityCandidate {
    pub name: String,
    pub name_ar: String,
    /// University ID.
    pub student_id: String,
    /// e.g. "••••••3456" (§9.2).
    pub national_id_masked: String,
    pub section: String,
    pub campus: String,
    pub track: String,
    pub seat: String,
    /// Posterior probability, 0..1.
    pub posterior: f32,
    pub channels: Vec<ChannelEvidence>,
}

/// Evidence of an identity item.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityEvidence {
    /// What the ID grid reads, `?` for an unreadable column.
    pub read_id: String,
    /// What the boxes above the grid read (handwriting channel).
    pub boxes_read: String,
    /// The writer-error hypothesis in words, e.g. "Columns 9 and 10 swapped".
    pub hypothesis: String,
    /// Top candidates, best first (at most 3 are shown, keys 1–3).
    pub candidates: Vec<IdentityCandidate>,
    /// Seat-map neighbours, e.g. "Seat B-14 · Noura Al-Harbi".
    pub neighbours: Vec<String>,
}

/// Evidence of a page- or exam-level (notice) item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeEvidence {
    pub headline: String,
    /// Key-value evidence lines.
    pub lines: Vec<(String, String)>,
    /// What "Acknowledge" will do, e.g. "Page goes to the rescan pull list".
    pub consequence: String,
}

/// Batch-confirm eligibility of the item's group (§13.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupConfirm {
    /// Open members of the group, including this item.
    pub size: u32,
    /// The open members, in queue order (the contact sheet shows their crops, §13.2).
    pub members: Vec<ItemId>,
    pub allowed: bool,
    /// Why not, when `!allowed`, e.g. "Open the contact sheet first".
    pub reason: String,
}

/// A queue entry (left pane).
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub id: ItemId,
    /// Primary text, e.g. "Q17" (mark items) or "Booklet 0277" (identity, notice items).
    pub label: String,
    /// Secondary text, e.g. "Form B" or "writer error".
    pub detail: String,
    /// 1-based question number; 0 for items that are not about a question.
    pub question: u32,
    pub form: String,
    /// Booklet serial, e.g. "0412".
    pub serial: String,
    pub severity: Severity,
    /// Per-option fills for the sparkline, 0..1 (empty for blind items).
    pub fills: Vec<f32>,
    pub kind: ReviewKind,
    pub deferred: bool,
}

/// Open items of one exception code, in queue order.
#[derive(Debug, Clone, PartialEq)]
pub struct ExceptionGroup {
    pub code: String,
    pub title: String,
    pub severity: Severity,
    pub items: Vec<QueueItem>,
}

/// The review queue of one lane.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewQueue {
    pub lane: Lane,
    pub lanes: Vec<LaneCount>,
    /// Ordered by severity (highest first), then code (§13.2).
    pub groups: Vec<ExceptionGroup>,
    /// Resolved and total exception items of the exam (all lanes).
    pub resolved: u32,
    pub total: u32,
    /// `None` when review times are not recorded.
    pub median_secs_per_item: Option<f64>,
    /// Audit-chain head for the status bar, abbreviated.
    pub audit_head: String,
    pub two_person_rule: bool,
    /// Why the exam has no queue to show right now (being graded, failed to open); `None`
    /// when the groups are the exam's queue, even an empty one.
    pub unavailable: Option<String>,
}

/// Everything the review frame shows for one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewItem {
    pub id: ItemId,
    pub code: String,
    pub code_title: String,
    /// e.g. "light", "erasure", "two candidates".
    pub sub_reason: String,
    pub severity: Severity,
    pub kind: ReviewKind,
    /// 1-based question number and its form (version).
    pub question: u32,
    pub form: String,
    pub serial: String,
    pub page: u32,
    pub dpi: u32,
    /// Registered crop: the answer row (mark items), the ID grid (identity items) or the page
    /// (notice items).
    pub crop: CropImage,
    /// The same region in the original (unregistered) scan, for `F2`.
    pub original: Option<CropImage>,
    pub bubbles: Vec<BubbleView>,
    /// Decision threshold and hysteresis band on the fill scale.
    pub threshold: f32,
    pub band: (f32, f32),
    /// `None` for blind items.
    pub suggestion: Option<Suggestion>,
    /// Fills of the student's other answered questions (top bubble), for "typical full fill".
    pub other_fills: Vec<f32>,
    pub typical_full_fill: f32,
    pub answered: u32,
    /// Accessible text summary, e.g. "Question 17, form B. B filled 0.82, D light 0.31.
    /// Suggestion B, engine." (§13.3).
    pub summary: String,
    pub group: GroupConfirm,
    /// A decision already recorded for this item (re-opened by undo), if any.
    pub current: Option<Decision>,
    /// Set for [`ReviewKind::Identity`] items.
    pub identity: Option<IdentityEvidence>,
    /// Set for [`ReviewKind::Notice`] items.
    pub notice: Option<NoticeEvidence>,
    /// Reason codes the core accepts when a decision overrides the suggestion (§13.4, §14.4),
    /// the item's family first; empty when the core asks for none.
    pub reasons: Vec<String>,
    /// Those of `reasons` that need a note saying why (`OTHER`, `SOLO_SELF_APPROVAL`, §14.4).
    pub reasons_needing_note: Vec<String>,
}

/// A review command (§13.3). Every one becomes an audited event in the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAction {
    /// Record what is marked. `accepted_suggestion` is true when the reviewer pressed
    /// "Accept suggestion" without choosing anything else (automation-bias metrics, §13.4).
    Resolve {
        decision: Decision,
        accepted_suggestion: bool,
        note: String,
    },
    /// Apply the decision to every open member of the item's group (§13.2 batch confirm).
    ApplyToGroup { decision: Decision },
    /// Identity items: the student number the reviewer typed (from the sheet's writing or the
    /// registrar), for a student who is not among the listed candidates; on the roster when
    /// the exam has one.
    SetStudent { student_id: String, note: String },
    /// Move to the end of the queue.
    Defer,
    /// Escalate to the instructor.
    Escalate,
    /// Add the sheet to the rescan pull list.
    AddToRescan,
}

/// Result of [`Backend::apply`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    /// Items closed or moved by the action.
    pub affected: Vec<ItemId>,
    /// The item to show next in the same lane, in queue order.
    pub next: Option<ItemId>,
}

/// One recorded review event, newest last. An identity decision for a student outside the
/// item's listed candidates (recorded by another client) reads `Candidate(u8::MAX)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub seq: u64,
    pub item: ItemId,
    pub question: u32,
    pub serial: String,
    pub action: ReviewAction,
    /// The reviewer's reason code, when one was given (§13.4).
    pub reason: Option<String>,
    /// True once compensated by an undo (the event itself is never rewritten, §4.7).
    pub undone: bool,
}

// ------------------------------------------------------------------------------------- results

/// One band of the grade scale with its count (§10.6).
#[derive(Debug, Clone, PartialEq)]
pub struct GradeBand {
    /// "A+".
    pub letter: String,
    /// "أ+".
    pub letter_ar: String,
    /// Band in percent, `[lo, hi)`; `hi = None` for the top band.
    pub lo: f64,
    pub hi: Option<f64>,
    /// Grade points on the 5.00 and 4.00 bases, when the scale publishes them.
    pub gpa5: Option<f64>,
    pub gpa4: Option<f64>,
    pub count: u32,
    pub fail: bool,
}

/// L6 status column (TP-154 A.1), e.g. DN / Denied entry / محروم.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusCount {
    pub code: String,
    pub label: String,
    pub label_ar: String,
    pub count: u32,
}

/// Score histogram over percent.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreHistogram {
    /// Bin width in percent (bins start at 0).
    pub bin_width: f64,
    pub counts: Vec<u32>,
    pub pass_mark: f64,
    pub mean: f64,
    /// SEM in percent points; the shell draws a ±1.96·SEM band at the pass mark.
    pub sem: f64,
}

/// Result statistics for the stat strip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultStats {
    pub students: u32,
    /// Students on the roster; 0 when the exam has none.
    pub roster: u32,
    pub absent: u32,
    pub mean_pct: f64,
    pub median_pct: f64,
    pub sd_pct: f64,
    pub pass_rate: f64,
    /// Cronbach α with its Feldt 95 % CI (`None`: too few students or no variance).
    pub alpha: Option<f64>,
    pub alpha_ci: Option<(f64, f64)>,
    /// SEM in points (`None` without α).
    pub sem_points: Option<f64>,
    pub open_errors: u32,
}

/// Discrimination band of corrected item-rest r (§12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscriminationBand {
    Flaw,
    Review,
    Marginal,
    Acceptable,
    Good,
}

impl DiscriminationBand {
    pub fn from_r(r: f64) -> Self {
        if r < 0.0 {
            Self::Flaw
        } else if r < 0.10 {
            Self::Review
        } else if r < 0.20 {
            Self::Marginal
        } else if r < 0.30 {
            Self::Acceptable
        } else {
            Self::Good
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Self::Flaw => "flaw",
            Self::Review => "review",
            Self::Marginal => "marginal",
            Self::Acceptable => "acceptable",
            Self::Good => "good",
        }
    }
    /// Filled steps of the 5-step meter.
    pub fn steps(self) -> u8 {
        match self {
            Self::Flaw => 0,
            Self::Review => 1,
            Self::Marginal => 2,
            Self::Acceptable => 3,
            Self::Good => 5,
        }
    }
}

/// How far an item-analysis flag survives sampling noise (§12.2 "Flag stability").
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FlagStability {
    /// The stability job has not reported yet ("computing…" in the table).
    #[default]
    Computing,
    /// The rule fires in most bootstrap resamples (≥ 50 %) and holds at both ends of its 95 %
    /// CI (N ≥ 1,000: at both ends of the Fisher-z CI; `share` is then `None`).
    High { share: Option<f64>, note: String },
    /// Low confidence *(proposed)*: the rule fires in fewer than half of the resamples, or
    /// fails at one end of its CI. `note` says which, e.g. "fires in 31 % of 1,000 bootstrap
    /// resamples; fails at the upper end of its 95 % CI".
    Low { share: Option<f64>, note: String },
    /// Not assessed: the rule is not one of ranks 1–4 (e.g. "too easy").
    NotAssessed,
}

/// An item-analysis flag (§12.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemFlag {
    /// e.g. "Probable mis-key: D" (the rule's statement, without the stability account).
    pub message: String,
    /// e.g. Some("SCO-001").
    pub code: Option<String>,
    pub tone: Tone,
    /// Low-confidence (unstable) flags are marked "proposed" (§12.2).
    pub stable: bool,
    /// Rank of the rule (1 = probable mis-key … ; §12.2 "ranked").
    pub rank: u8,
    /// The stability of the flag, once the stability job has run.
    pub stability: FlagStability,
}

/// One row of the item-analysis table.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStats {
    pub question: u32,
    /// Row label when the question number alone is ambiguous, e.g. "B-Q03" for a master item
    /// of form B; empty = "Q" and the question number.
    pub label: String,
    /// Difficulty p (`None`: not computed, e.g. nobody was scored on the item).
    pub p: Option<f64>,
    /// Corrected item-rest r and its 95 % CI (`None`: not computable, e.g. too few students
    /// or no score variance; never shown as 0).
    pub r_ir: Option<f64>,
    pub r_ci: Option<(f64, f64)>,
    /// The band of `r_ir`, when there is one.
    pub band: Option<DiscriminationBand>,
    /// Share choosing each option (omit excluded), in option order.
    pub option_shares: Vec<f64>,
    /// Keyed option, shown only when the user reveals the key.
    pub keyed: Option<u8>,
    pub flags: Vec<ItemFlag>,
}

/// Where a readiness check's action button leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTarget {
    /// The review queue; `Some(lane)` switches to that lane first.
    Review(Option<Lane>),
    /// The live batch (reconciliation, rescan pull list).
    Scan,
    /// Exam setup (key, mapping).
    Setup,
    /// Learning outcomes.
    Outcomes,
}

/// A release-readiness check (DESIGN §9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessCheck {
    pub tone: Tone,
    pub text: String,
    /// Action label for the check's button, e.g. "Open"; empty for none.
    pub action: String,
    /// Where the action leads; `None` when there is no action.
    pub target: Option<CheckTarget>,
}

/// Results of an exam (grades, histogram, item analysis).
#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    pub exam: ExamId,
    pub key_label: String,
    pub stats: ResultStats,
    pub histogram: ScoreHistogram,
    /// e.g. "Saudi scale (CUA 1444/9/1) · GPA basis 5.00".
    pub scale_label: String,
    pub grades: Vec<GradeBand>,
    pub statuses: Vec<StatusCount>,
    pub items: Vec<ItemStats>,
    pub option_labels: Vec<String>,
    pub readiness: Vec<ReadinessCheck>,
    /// `Some(reason)` while release is blocked, e.g. "38 ERROR open".
    pub release_blocked: Option<String>,
    /// `Some(reason)` when the Blackboard grade-center upload cannot be written (an inferred
    /// key, no roster, §16.1); `None` when [`Backend::export_blackboard`] can write it.
    pub blackboard_blocked: Option<String>,
}

// ------------------------------------------------------------------------------------- outcomes

/// Class method of the attainment policy (§11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassMethod {
    Average,
    ThresholdProportion,
    PerQuestion,
    /// Attainment levels 1–3 against level cut-offs (§11.3).
    Levels,
    /// ABET EAMU bands.
    Eamu,
}

/// Verdict of one CLO under the policy (§11.3, §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Attained,
    NotAttained,
    /// SCO-014 thin evidence or SCO-023 small n: shown, never decided silently.
    InsufficientEvidence,
}

/// Attainment policy summary printed with every result.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyView {
    pub class_method: ClassMethod,
    /// Student threshold τ, 0..1.
    pub tau: f64,
    /// Class target T, 0..1.
    pub target: f64,
    /// e.g. "point" or "ci_lower".
    pub verdict_basis: String,
    /// Which mappings are evidence, e.g. "approved mappings only".
    pub mapping_basis: String,
    /// e.g. "Policy v3 · sha384 9a1c…".
    pub version: String,
}

/// One CLO's attainment row.
#[derive(Debug, Clone, PartialEq)]
pub struct CloAttainment {
    pub code: String,
    pub title: String,
    pub title_ar: String,
    pub domain: NqfDomain,
    /// Target T for this CLO (per-CLO override allowed).
    pub target: f64,
    /// Average μ and its 95 % CI.
    pub average: f64,
    pub average_ci: (f64, f64),
    /// Threshold-proportion π and its Wilson 95 % CI.
    pub proportion: f64,
    pub proportion_ci: (f64, f64),
    pub n: u32,
    pub verdict: Verdict,
    /// The CI of the deciding statistic contains T ("statistically indistinguishable", §11.3).
    pub ci_contains_target: bool,
    /// Evidence summary, e.g. "Q3–5, Q12 · 4 items · Σw 3.5".
    pub mapping_note: String,
}

/// One TP-154 B.1 row, cells formatted by the report package (§11.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tp154Row {
    pub domain: NqfDomain,
    /// e.g. "1.1".
    pub number: String,
    pub clo: String,
    pub plo: String,
    pub methods: String,
    pub targeted: String,
    pub actual: String,
    pub comment: String,
}

/// Learning outcomes of an exam.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcomes {
    pub exam: ExamId,
    pub policy: PolicyView,
    pub clos: Vec<CloAttainment>,
    pub tp154_b1: Vec<Tp154Row>,
    /// Classification label of the report, e.g. "Restricted · مقيد".
    pub classification: String,
}

// ------------------------------------------------------------------------------------- trait

// ------------------------------------------------------------------------------------- key sheets

/// How one row of a key sheet stands (§10.1): only a confidently read single option is a key
/// entry; every other row waits for the instructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRowStatus {
    /// One option read confidently (to be confirmed with the sheet).
    Read,
    /// Decided by the instructor (recorded with the confirmation).
    Decided,
    /// Several marks (SCO-027): which answers count?
    MultiMark,
    /// No mark (SCO-003).
    Blank,
    /// A mark on an option the exam does not use (SCO-004).
    NotPrinted,
    /// A mark not read confidently.
    Uncertain,
    /// Two key sheets of the version disagree.
    Disagree,
}

impl KeyRowStatus {
    /// Short status word.
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Decided => "decided by you",
            Self::MultiMark => "several marks",
            Self::Blank => "no mark",
            Self::NotPrinted => "option not used",
            Self::Uncertain => "not read clearly",
            Self::Disagree => "key sheets disagree",
        }
    }
    /// True while the row waits for the instructor's decision.
    pub fn open(self) -> bool {
        !matches!(self, Self::Read | Self::Decided)
    }
}

/// One question of a version's key read from key sheets.
#[derive(Debug, Clone, PartialEq)]
pub struct KeySheetRow {
    /// 1-based question.
    pub question: u32,
    /// What the sheet shows: `B`, `A+C`, `blank`, `B?` (not read confidently), `?`.
    pub read: String,
    /// The key entry the row gives: `B`, `A|C`, `*` (void), or empty while undecided.
    pub answer: String,
    pub status: KeyRowStatus,
    /// Why the row needs a decision and what to do (or the recognizer's codes).
    pub detail: String,
    /// The current key's entry when it differs from the sheet (the exam already has a key).
    pub key_file: Option<String>,
    /// Bubble fills δ, option order.
    pub fills: Vec<f32>,
    /// The options the sheet shows marked (offered as "accept these" on a multi-mark row).
    pub marked: Vec<u8>,
    /// The bubble row as scanned, straightened along its own axis, with the mark overlays
    /// drawn on it (paper colours, DESIGN §4); `None` when the page image is unavailable.
    pub crop: Option<CropImage>,
}

/// One key-sheet scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySheetScan {
    /// File and page, e.g. `key-A.png page 1`.
    pub label: String,
    /// Named as a key sheet ("named") or found among the students' scans ("found in the stack").
    pub origin: String,
    /// The student-number grid as read (the key-sheet marker, e.g. `0000000000`).
    pub id_read: String,
    /// The version bubbled on the sheet.
    pub version: String,
    /// Registration summary, e.g. "matched 684/687 bubbles, rms 0.41 px".
    pub registration: String,
    /// SHA-384 of the file, abbreviated.
    pub digest: String,
    /// Why the sheet gives no key (R10: what to do), if so.
    pub refusal: Option<String>,
}

/// One version's key read from key sheets.
#[derive(Debug, Clone, PartialEq)]
pub struct KeySheetVersion {
    /// Version letter.
    pub version: String,
    /// Index of the version in the exam's forms (what [`KeyRowDecision::version`] names).
    pub form: usize,
    pub sheets: Vec<KeySheetScan>,
    pub rows: Vec<KeySheetRow>,
    /// Rows waiting for the instructor.
    pub open: u32,
    /// "by … at …" once confirmed.
    pub confirmed: Option<String>,
}

/// The key read from an exam's key sheets, as the instructor confirms it (§10.1: key-sheet
/// responses are never auto-accepted). Either the key sheets graded with the exam (the run is
/// held until the confirmation) or key-sheet files read for an exam that already has a key
/// (`preview`: nothing changes until the confirmation replaces the key).
#[derive(Debug, Clone, PartialEq)]
pub struct KeySheetSummary {
    /// Where the key comes from, e.g. "Key sheets graded with the exam" or "Read from 2 key-sheet
    /// files".
    pub source: String,
    /// Read from files for this exam, not part of it yet.
    pub preview: bool,
    /// True while a version waits for the confirmation.
    pub pending: bool,
    /// A key file replaced the key sheets (they are listed, not used).
    pub superseded: bool,
    pub versions: Vec<KeySheetVersion>,
    /// Key sheets that give no key, with the reason.
    pub unusable: Vec<KeySheetScan>,
    /// Versions without a key sheet and where their key comes from.
    pub other_versions: Vec<(String, String)>,
    /// What confirming does (shown beside the Confirm button).
    pub consequence: String,
    /// Rule notes (e.g. a marker not used in the stack).
    pub notes: Vec<String>,
}

/// The instructor's decision on one key-sheet row, sent with the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRowDecision {
    /// Version (index into the exam's forms).
    pub version: usize,
    /// 1-based question.
    pub question: u32,
    /// `B`, `A|C` (several accepted answers) or `*` (void the question).
    pub answer: String,
}

/// A new exam as typed in the New-exam form (plain text; the backend validates it with the
/// same rules as `omr grade`). Blank optional fields take the sheet's defaults; a blank key
/// file means the key is inferred (provisional, never releasable, replaceable from Setup).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewExam {
    pub title: String,
    pub course: String,
    /// Scan files or folders, separated by `;` or new lines.
    pub scans: String,
    /// A template ID (`iau-ccsit-v1`) or the spec file of an own sheet.
    pub sheet: String,
    pub questions: String,
    /// Options per question (blank: the sheet's).
    pub options: String,
    /// Form letters, e.g. `A,B` (blank: the key file's, else the sheet's).
    pub forms: String,
    pub key: String,
    pub roster: String,
    pub clo: String,
    /// Key-sheet files (the instructor's filled answer sheets), separated by `;` (optional):
    /// graded with the exam as named key sheets, held until confirmed in Setup (§10.1).
    pub key_sheets: String,
}

/// What the shell needs from the application core. Queries are cheap snapshots; commands
/// validate, record an audited event and return the new state the shell should show.
///
/// The shell calls everything on the UI thread. A core with worker threads hands their results
/// over in [`Backend::pump`], which the shell calls about twice a second.
pub trait Backend {
    // ---- exams
    fn dashboard(&self) -> Dashboard;
    /// The exam the user worked on last (the shell opens it first).
    fn default_exam(&self) -> Option<ExamId>;

    // ---- setup
    fn exam_setup(&self, exam: &ExamId) -> Option<ExamSetup>;
    /// Sets the key of one question in one version (`answer` = "", "B" or "A,C").
    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), BackendError>;
    /// Sets or clears (`None`) the weight of item `question` toward CLO `clo` (index).
    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), BackendError>;
    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), BackendError>;

    // ---- scan
    fn batch_progress(&self, exam: &ExamId) -> Option<BatchProgress>;

    // ---- review
    fn review_queue(&self, exam: &ExamId, lane: Lane) -> ReviewQueue;
    fn review_item(&self, exam: &ExamId, item: ItemId) -> Option<ReviewItem>;
    fn apply(
        &mut self,
        exam: &ExamId,
        item: ItemId,
        action: ReviewAction,
    ) -> Result<ApplyOutcome, BackendError>;
    /// [`Backend::apply`] with the reviewer's reason code (one of [`ReviewItem::reasons`]); the
    /// core requires one when the decision overrides its suggestion (§13.4). The default drops
    /// the reason (a backend that asks for none).
    fn apply_with_reason(
        &mut self,
        exam: &ExamId,
        item: ItemId,
        action: ReviewAction,
        reason: Option<String>,
    ) -> Result<ApplyOutcome, BackendError> {
        let _ = reason;
        self.apply(exam, item, action)
    }
    /// True when committing `decision` on `item` overrides the core's suggestion, so a reason
    /// is required first (§13.4). The default asks for none.
    fn needs_reason(&self, exam: &ExamId, item: ItemId, decision: Decision) -> bool {
        let _ = (exam, item, decision);
        false
    }
    /// [`Backend::needs_reason`] for a typed student number ([`ReviewAction::SetStudent`]).
    fn needs_reason_for_student(&self, exam: &ExamId, item: ItemId, student_id: &str) -> bool {
        let _ = (exam, item, student_id);
        false
    }
    /// Compensates the latest not-yet-undone event; returns the item it re-opened.
    fn undo(&mut self, exam: &ExamId) -> Result<Option<ItemId>, BackendError>;
    /// Review events of the exam, oldest first.
    fn review_history(&self, exam: &ExamId) -> Vec<DecisionRecord>;

    // ---- results and outcomes
    fn results(&self, exam: &ExamId) -> Option<Results>;
    fn outcomes(&self, exam: &ExamId) -> Option<Outcomes>;

    /// Writes the exam's exports (results, item analysis, TP-154 tables, decisions) and returns
    /// where they went. The default refuses: a backend without a project store has nothing to
    /// export.
    fn export(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let _ = exam;
        Err(BackendError::Denied(
            "Exports need a graded project: open one with --project or --workspace".into(),
        ))
    }

    /// Writes the Blackboard Learn grade-center upload (§16.3), for an existing grade column
    /// when `column_id` is given (else Blackboard creates a new column), and returns where it
    /// went. The default refuses, as [`Backend::export`] does.
    fn export_blackboard(
        &mut self,
        exam: &ExamId,
        column_id: Option<&str>,
    ) -> Result<String, BackendError> {
        let _ = (exam, column_id);
        Err(BackendError::Denied(
            "The Blackboard upload needs a graded project: open one with --project or --workspace"
                .into(),
        ))
    }

    /// True for invented demo data: the shell shows a banner on every screen. The default is
    /// false.
    fn is_demo(&self) -> bool {
        false
    }

    /// True when a new exam can be graded from the New-exam form (a workspace to grade into,
    /// and no other run in progress).
    fn can_create_exam(&self) -> bool {
        false
    }

    /// Validates `form` and starts grading it in the background into a new folder of the
    /// workspace; progress arrives through [`Backend::pump`] as for an added batch. Returns
    /// the new exam and what was started. The default refuses.
    fn create_exam(&mut self, form: &NewExam) -> Result<(ExamId, String), BackendError> {
        let _ = form;
        Err(BackendError::Denied(
            "Creating an exam needs an exams folder: start omr-desktop with --workspace DIR".into(),
        ))
    }

    /// True when the exam's answer key can be replaced from a key file (a writable project).
    fn can_replace_key(&self, exam: &ExamId) -> bool {
        let _ = exam;
        false
    }

    /// Replaces the exam's whole answer key from a key file (`question,A,B[,points]`,
    /// `form,Q1,Q2,…` and the other layouts `omr grade --key` reads): a new key version, the
    /// exam rescored, the change audited (§10.1, §10.8). Returns what happened. The default
    /// refuses.
    fn replace_key(
        &mut self,
        exam: &ExamId,
        path: &std::path::Path,
    ) -> Result<String, BackendError> {
        let _ = (exam, path);
        Err(BackendError::Denied(
            "Replacing the key needs a graded project: open one with --project or --workspace"
                .into(),
        ))
    }

    /// The key read from the exam's key sheets, waiting for (or past) the instructor's
    /// confirmation: the key sheets graded with the exam, else key-sheet files read for it with
    /// [`Backend::read_key_sheets`]. `None` when there are none.
    fn key_sheets(&self, exam: &ExamId) -> Option<KeySheetSummary> {
        let _ = exam;
        None
    }

    /// Why the exam is held before anything is scored (e.g. a key from key sheets that waits
    /// for the confirmation), with what to do; `None` when nothing holds it.
    fn hold(&self, exam: &ExamId) -> Option<String> {
        let _ = exam;
        None
    }

    /// Reads key-sheet files (the instructor's filled answer sheets) for the exam with the
    /// same recognizer as the students' sheets; the key they give is shown for confirmation
    /// ([`Backend::key_sheets`]) and changes nothing until [`Backend::confirm_key_sheets`].
    /// The default refuses.
    fn read_key_sheets(
        &mut self,
        exam: &ExamId,
        inputs: &[std::path::PathBuf],
    ) -> Result<String, BackendError> {
        let _ = (exam, inputs);
        Err(BackendError::Denied(
            "Reading a key from answer sheets needs a graded project: open one with --project \
             or --workspace"
                .into(),
        ))
    }

    /// Confirms the key read from key sheets, one version (`Some(form)`) or every version
    /// waiting (`None`), with the instructor's decisions on the rows that need one. Recorded
    /// as an audited event with the confirming operator; the last version confirmed puts the
    /// key in force and the exam is scored. A row left undecided refuses it, naming the row.
    fn confirm_key_sheets(
        &mut self,
        exam: &ExamId,
        version: Option<usize>,
        decisions: &[KeyRowDecision],
    ) -> Result<String, BackendError> {
        let _ = (exam, version, decisions);
        Err(BackendError::Denied(
            "Confirming a key needs a graded project: open one with --project or --workspace"
                .into(),
        ))
    }

    /// Drops key-sheet files read with [`Backend::read_key_sheets`] and not confirmed (nothing
    /// was changed). The default has none to drop.
    fn discard_key_sheets(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let _ = exam;
        Err(BackendError::Invalid(
            "No key-sheet files were read for this exam".into(),
        ))
    }

    /// Releases the exam's results (§13.4): refused while anything blocks release; the
    /// released score run is frozen in the audit log and the exports, the Blackboard upload
    /// with them, are written. Returns what happened. The default refuses.
    fn release(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let _ = exam;
        Err(BackendError::Denied(
            "Releasing needs a graded project: open one with --project or --workspace".into(),
        ))
    }

    /// Starts adding a later batch of scans (files or folders) to a graded exam (§8.1): the
    /// exam is graded again over every page, in the background, and review decisions carry
    /// over. Progress arrives through [`Backend::pump`]; returns what was started. The default
    /// refuses.
    fn add_scans(
        &mut self,
        exam: &ExamId,
        inputs: &[std::path::PathBuf],
    ) -> Result<String, BackendError> {
        let _ = (exam, inputs);
        Err(BackendError::Denied(
            "Adding scans needs a graded project: open one with --project or --workspace".into(),
        ))
    }

    /// Pauses (`true`) or resumes (`false`) the exam's running batch. The default refuses:
    /// a core without a capture agent cannot pause anything.
    fn pause_batch(&mut self, exam: &ExamId, paused: bool) -> Result<(), BackendError> {
        let _ = (exam, paused);
        Err(BackendError::Denied(
            "Pausing belongs to the capture agent, which is not connected".into(),
        ))
    }

    // ---- live updates
    /// Folds finished background work into the view-models. Returns true when anything the
    /// shell shows may have changed. The default does nothing.
    fn pump(&mut self) -> bool {
        false
    }

    /// Blocks until background analyses that are running (flag stability) have reported and
    /// folds them, as [`Backend::pump`] does: for headless screenshots, which must not catch
    /// "computing…". Returns true when anything changed. The default has nothing to wait for.
    fn settle(&mut self) -> bool {
        false
    }

    /// Exams whose project was reloaded since the last call (a grading run or an added batch
    /// finished): their review item IDs are new, so the shell drops its selection. The default
    /// reloads nothing.
    fn take_reloaded(&mut self) -> Vec<ExamId> {
        Vec::new()
    }
}
