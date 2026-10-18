//! View-model queries for the desktop shell (§4.7). The shapes mirror
//! `shell-slint/src/backend.rs` field for field where practical, so the shell's `Backend`
//! implementation over [`ExamProject`] is a thin adapter: plain Rust, no toolkit types, lists
//! in display order, key-isolation in the review lanes (§13.2, §19.3).

use crate::app::ExamProject;
use crate::export::response_text;
use crate::model::{FormSource, PageStatus, ResponseStatus};
use crate::pipeline::Progress;
use crate::review::{Decision, ItemKind, ItemState, Lane, OverlayState, ReviewItem};
use num_traits::ToPrimitive;
use omr_model::Severity;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Open exceptions by severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    /// INFO.
    pub info: u32,
    /// WARN.
    pub warn: u32,
    /// ERROR.
    pub error: u32,
    /// BLOCKER.
    pub blocker: u32,
}

/// Exam lifecycle status (dashboard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExamStatus {
    /// No pages yet.
    Setup,
    /// Pages arriving.
    Scanning,
    /// Open ERROR/BLOCKER.
    InReview,
    /// Releasable.
    Ready,
    /// Released.
    Released,
}

/// One dashboard row (shell `ExamSummary`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExamSummary {
    /// Exam ID.
    pub id: String,
    /// Course code.
    pub course_code: String,
    /// Course title.
    pub course_title: String,
    /// Arabic course title.
    pub course_title_ar: String,
    /// Assessment.
    pub assessment: String,
    /// Term label.
    pub term: String,
    /// ISO date.
    pub date: String,
    /// Version letters.
    pub versions: Vec<String>,
    /// Status.
    pub status: ExamStatus,
    /// Sheets scanned.
    pub sheets_scanned: u32,
    /// Sheets expected (roster size, else scanned).
    pub sheets_expected: u32,
    /// Open exceptions.
    pub open_exceptions: SeverityCounts,
    /// e.g. "Key v1 · INFERRED".
    pub key_status: String,
    /// Items with an approved CLO mapping.
    pub mapped_items: u32,
    /// Items.
    pub total_items: u32,
    /// Roster count.
    pub roster_count: u32,
    /// Sections.
    pub sections: Vec<String>,
    /// Batch ID.
    pub batch_id: String,
}

/// Template info (shell `TemplateInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateInfo {
    /// e.g. "iau-ccsit-v1".
    pub id: String,
    /// Legacy (true) or generated.
    pub legacy: bool,
    /// Display name.
    pub name: String,
    /// Questions used.
    pub questions: u32,
    /// Questions printed.
    pub capacity: u32,
    /// Option labels.
    pub option_labels: Vec<String>,
    /// Versions.
    pub versions: Vec<String>,
    /// Cancel mode label.
    pub cancel_mode: String,
    /// Working dpi.
    pub dpi: u32,
    /// Page identity.
    pub page_identity: String,
    /// Template digest, abbreviated.
    pub digest: String,
}

/// One version's key (shell `VersionKey`): "B", "A,C", "" per question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionKey {
    /// Version letter.
    pub version: String,
    /// Entries.
    pub answers: Vec<String>,
}

/// A CLO (shell `CloDef`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloDef {
    /// Code.
    pub code: String,
    /// Title.
    pub title: String,
    /// Arabic title.
    pub title_ar: String,
    /// Domain letter K/S/V.
    pub domain: String,
    /// PLO codes.
    pub plo: String,
}

/// One mapping row (shell `MappingRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct MappingRow {
    /// 1-based question.
    pub question: u32,
    /// Points.
    pub points: f64,
    /// Per CLO: weight and note.
    pub cells: Vec<(Option<f64>, String)>,
}

/// Exam setup (shell `ExamSetup`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExamSetup {
    /// Exam ID.
    pub exam: String,
    /// Template.
    pub template: TemplateInfo,
    /// Keys.
    pub keys: Vec<VersionKey>,
    /// e.g. "Key v1 · INFERRED".
    pub key_status: String,
    /// False for inferred or missing keys.
    pub key_published: bool,
    /// CLOs.
    pub clos: Vec<CloDef>,
    /// Split mode.
    pub mapping_split: bool,
    /// Mapping.
    pub mapping: Vec<MappingRow>,
    /// Lints (code, severity, message).
    pub lints: Vec<(String, Severity, String)>,
}

/// Thumbnail state (shell `ThumbState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbState {
    /// Auto-accepted.
    Auto,
    /// Needs review.
    Review(String, Severity),
    /// Rejected at capture.
    Rejected(String, Severity),
    /// Being read.
    Reading,
    /// Queued.
    Queued,
}

/// Live batch progress (shell `BatchProgress`), folded from [`Progress`] events.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchProgress {
    /// e.g. "Batch 1 of 1".
    pub batch_label: String,
    /// Source.
    pub source: String,
    /// Running.
    pub running: bool,
    /// Pages done.
    pub pages_done: u32,
    /// Pages total.
    pub pages_total: u32,
    /// Throughput.
    pub pages_per_sec: f64,
    /// Spec floor (40 pages/s, §2.4).
    pub floor_pages_per_sec: f64,
    /// Gauge maximum.
    pub max_pages_per_sec: f64,
    /// Share of questions auto-accepted.
    pub auto_accepted: f64,
    /// Abstention rate.
    pub abstention_rate: f64,
    /// Time to the first result.
    pub first_result: Duration,
    /// Remaining (estimate).
    pub remaining: Duration,
    /// In flight.
    pub in_flight: u32,
    /// Queued.
    pub queued: u32,
    /// Stage counters.
    pub stages: Vec<(String, u32)>,
    /// Open codes (code, severity, count), by severity then count.
    pub exceptions: Vec<(String, Severity, u32)>,
    /// Thumbnails, newest first (page ID, state).
    pub thumbnails: Vec<(String, ThumbState)>,
    /// Activity lines, newest first.
    pub activity: Vec<String>,
    /// Workers.
    pub workers: u32,
}

/// Folds progress events into [`BatchProgress`] snapshots (≤ 20 Hz coalescing is the UI's).
#[derive(Clone, Debug, Default)]
pub struct ProgressTracker {
    done: u32,
    total: u32,
    workers: u32,
    elapsed_ms: f64,
    first_ms: Option<f64>,
    questions: u64,
    auto: u64,
    codes: BTreeMap<(String, Severity), u32>,
    thumbs: Vec<(String, ThumbState)>,
    stage: String,
    finished: bool,
}

impl ProgressTracker {
    /// Folds one event.
    pub fn push(&mut self, p: &Progress) {
        match p {
            Progress::Started { pages, workers } => {
                self.total = *pages as u32;
                self.workers = *workers as u32;
                self.stage = "page stage".into();
            }
            Progress::Page {
                done,
                elapsed_ms,
                page,
                ..
            } => {
                self.done = *done as u32;
                self.elapsed_ms = *elapsed_ms;
                self.first_ms.get_or_insert(*elapsed_ms);
                self.questions += page.questions as u64;
                self.auto += page.auto_accepted as u64;
                let st = match &page.worst_code {
                    Some((c, s)) if *s == Severity::Blocker && page.questions == 0 => {
                        ThumbState::Rejected(c.clone(), *s)
                    }
                    Some((c, s)) if *s >= Severity::Error => ThumbState::Review(c.clone(), *s),
                    _ if page.auto_accepted < page.questions => {
                        ThumbState::Review("MRK-001".into(), Severity::Error)
                    }
                    _ => ThumbState::Auto,
                };
                if let Some((c, s)) = &page.worst_code {
                    *self.codes.entry((c.clone(), *s)).or_default() += 1;
                }
                self.thumbs.insert(0, (page.page_id.clone(), st));
                self.thumbs.truncate(60);
            }
            Progress::Stage(s) => self.stage = (*s).to_string(),
            Progress::Finished { total_ms } => {
                self.finished = true;
                self.elapsed_ms = *total_ms;
                self.stage = "finished".into();
            }
        }
    }

    /// The current snapshot.
    pub fn snapshot(&self) -> BatchProgress {
        let secs = self.elapsed_ms / 1e3;
        let pps = if secs > 0.0 {
            f64::from(self.done) / secs
        } else {
            0.0
        };
        let left = self.total.saturating_sub(self.done);
        let mut exceptions: Vec<(String, Severity, u32)> = self
            .codes
            .iter()
            .map(|((c, s), n)| (c.clone(), *s, *n))
            .collect();
        exceptions.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        let ar = if self.questions == 0 {
            0.0
        } else {
            1.0 - self.auto as f64 / self.questions as f64
        };
        BatchProgress {
            batch_label: "Batch 1 of 1".into(),
            source: "file import".into(),
            running: !self.finished,
            pages_done: self.done,
            pages_total: self.total,
            pages_per_sec: pps,
            floor_pages_per_sec: 40.0,
            max_pages_per_sec: 120.0,
            auto_accepted: 1.0 - ar,
            abstention_rate: ar,
            first_result: Duration::from_secs_f64(self.first_ms.unwrap_or(0.0) / 1e3),
            remaining: if pps > 0.0 {
                Duration::from_secs_f64(f64::from(left) / pps)
            } else {
                Duration::ZERO
            },
            in_flight: left.min(self.workers),
            queued: left.saturating_sub(self.workers),
            stages: vec![("read".into(), self.done), ("queued".into(), left)],
            exceptions,
            thumbnails: self.thumbs.clone(),
            activity: vec![format!(
                "{} · {}/{} pages",
                self.stage, self.done, self.total
            )],
            workers: self.workers,
        }
    }
}

/// A bubble drawn on the crop (shell `BubbleView`).
#[derive(Clone, Debug, PartialEq)]
pub struct BubbleView {
    /// Label.
    pub label: String,
    /// Centre x (crop px).
    pub cx: f32,
    /// Centre y (crop px).
    pub cy: f32,
    /// Radius (crop px).
    pub r: f32,
    /// Fill 0..1.
    pub fill: f32,
    /// State.
    pub state: OverlayState,
}

/// RGBA crop (shell `CropImage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CropImage {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Pixels.
    pub rgba: Arc<[u8]>,
}

/// Queue entry (shell `QueueItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    /// Item ID.
    pub id: u64,
    /// e.g. "Q17" or "Booklet 0277".
    pub label: String,
    /// e.g. "Form B".
    pub detail: String,
    /// 1-based question, 0 when not about a question.
    pub question: u32,
    /// Form letter.
    pub form: String,
    /// Booklet serial.
    pub serial: String,
    /// Severity.
    pub severity: Severity,
    /// Fills.
    pub fills: Vec<f32>,
    /// Kind.
    pub kind: ItemKind,
    /// Deferred.
    pub deferred: bool,
}

/// Items of one code (shell `ExceptionGroup`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionGroup {
    /// Code.
    pub code: String,
    /// Title.
    pub title: String,
    /// Severity.
    pub severity: Severity,
    /// Items in queue order.
    pub items: Vec<QueueItem>,
}

/// One lane's queue (shell `ReviewQueue`).
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewQueue {
    /// Lane.
    pub lane: Lane,
    /// Open counts per lane.
    pub lanes: Vec<(Lane, u32)>,
    /// Groups by severity then code.
    pub groups: Vec<ExceptionGroup>,
    /// Resolved items (all lanes).
    pub resolved: u32,
    /// Items (all lanes).
    pub total: u32,
    /// Audit-chain head, abbreviated.
    pub audit_head: String,
}

/// Batch-confirm eligibility (shell `GroupConfirm`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupConfirm {
    /// Open members.
    pub size: u32,
    /// Members in queue order.
    pub members: Vec<u64>,
    /// Allowed.
    pub allowed: bool,
    /// Why not.
    pub reason: String,
}

/// Everything the review frame shows (shell `ReviewItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewItemView {
    /// ID.
    pub id: u64,
    /// Code.
    pub code: String,
    /// Title.
    pub code_title: String,
    /// Sub-reason.
    pub sub_reason: String,
    /// Severity.
    pub severity: Severity,
    /// Kind.
    pub kind: ItemKind,
    /// 1-based question (0 = none).
    pub question: u32,
    /// Form letter.
    pub form: String,
    /// Serial.
    pub serial: String,
    /// Page within the source (1-based).
    pub page: u32,
    /// Working dpi.
    pub dpi: u32,
    /// Crop.
    pub crop: CropImage,
    /// Bubbles.
    pub bubbles: Vec<BubbleView>,
    /// Threshold on the fill scale.
    pub threshold: f32,
    /// Hysteresis band.
    pub band: (f32, f32),
    /// Engine suggestion (never key-derived).
    pub suggestion: Option<Decision>,
    /// Top fills of the student's other answered questions.
    pub other_fills: Vec<f32>,
    /// Median of `other_fills`.
    pub typical_full_fill: f32,
    /// Answered questions.
    pub answered: u32,
    /// Accessible summary.
    pub summary: String,
    /// Group.
    pub group: GroupConfirm,
    /// Recorded decision.
    pub current: Option<Decision>,
    /// Identity evidence.
    pub identity: Option<IdentityView>,
    /// Notice evidence (page-, booklet- and exam-level items, forms).
    pub notice: Option<NoticeView>,
}

/// Identity evidence (shell `IdentityEvidence`).
#[derive(Clone, Debug, PartialEq)]
pub struct IdentityView {
    /// What the grid reads (`?` uncertain, `_` blank).
    pub read_id: String,
    /// Candidates (student key, posterior), best first.
    pub candidates: Vec<(String, f32)>,
}

/// Notice evidence (shell `NoticeEvidence`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeView {
    /// Headline.
    pub headline: String,
    /// Key-value lines.
    pub lines: Vec<(String, String)>,
    /// What acknowledging does.
    pub consequence: String,
}

/// One decision record (shell `DecisionRecord`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionRecord {
    /// Sequence.
    pub seq: u64,
    /// Item.
    pub item: u64,
    /// Question (1-based, 0 = none).
    pub question: u32,
    /// Serial.
    pub serial: String,
    /// Action, as text.
    pub action: String,
    /// Undone.
    pub undone: bool,
}

/// A grade band with its count (shell `GradeBand`).
#[derive(Clone, Debug, PartialEq)]
pub struct GradeBand {
    /// Letter.
    pub letter: String,
    /// Arabic letter.
    pub letter_ar: String,
    /// Lower bound (%).
    pub lo: f64,
    /// Upper bound (%), `None` for the top band.
    pub hi: Option<f64>,
    /// Count.
    pub count: u32,
    /// Fail band.
    pub fail: bool,
}

/// Item statistics row (shell `ItemStats`).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemStats {
    /// 1-based master item.
    pub question: u32,
    /// Label.
    pub label: String,
    /// p.
    pub p: f64,
    /// r_ir.
    pub r_ir: f64,
    /// CI.
    pub r_ci: (f64, f64),
    /// Option shares.
    pub option_shares: Vec<f64>,
    /// Flags (message, code).
    pub flags: Vec<(String, Option<String>)>,
}

/// Results (shell `Results`).
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    /// e.g. "Key v1 · INFERRED".
    pub key_label: String,
    /// Students scored.
    pub students: u32,
    /// Mean %.
    pub mean_pct: f64,
    /// Median %.
    pub median_pct: f64,
    /// SD %.
    pub sd_pct: f64,
    /// Pass rate.
    pub pass_rate: f64,
    /// α and CI.
    pub alpha: Option<(f64, (f64, f64))>,
    /// Histogram counts over 10 % bins.
    pub histogram: Vec<u32>,
    /// Grade bands.
    pub grades: Vec<GradeBand>,
    /// Items.
    pub items: Vec<ItemStats>,
    /// Option labels.
    pub option_labels: Vec<String>,
    /// Readiness lines (ok, text).
    pub readiness: Vec<(bool, String)>,
    /// Release blocked reason.
    pub release_blocked: Option<String>,
    /// Roster students without a booklet (absent; 0 without a roster, §9.6).
    pub absent: u32,
}

/// CLO attainment row (shell `CloAttainment`).
#[derive(Clone, Debug, PartialEq)]
pub struct CloRow {
    /// Code.
    pub code: String,
    /// Title.
    pub title: String,
    /// Target.
    pub target: f64,
    /// Average μ.
    pub average: Option<f64>,
    /// Proportion π.
    pub proportion: Option<f64>,
    /// N.
    pub n: u32,
    /// Attained (`None`: insufficient evidence).
    pub attained: Option<bool>,
    /// CI contains the target.
    pub indistinguishable: bool,
}

/// Outcomes (shell `Outcomes`).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcomes {
    /// Class method.
    pub method: String,
    /// τ.
    pub tau: f64,
    /// T.
    pub target: f64,
    /// Rows.
    pub clos: Vec<CloRow>,
    /// TP-154 B.1 rows: number, CLO, PLO, methods, targeted, actual, comment.
    pub tp154_b1: Vec<[String; 7]>,
    /// Classification label.
    pub classification: String,
}

/// The exams dashboard of a workspace folder: every graded exam below `root` (folders holding
/// `project.omrproj`), newest snapshot first; unreadable projects are skipped.
pub fn list_exams(root: &std::path::Path) -> Vec<(std::path::PathBuf, ExamSummary)> {
    let mut out: Vec<(std::path::PathBuf, String, ExamSummary)> = walkdir::WalkDir::new(root)
        .max_depth(3)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir() && e.file_name() == "project.omrproj")
        .filter_map(|e| {
            let p = ExamProject::open_read_only(e.path()).ok()?;
            Some((
                e.into_path(),
                p.session.snapshot_utc.clone(),
                p.summary_vm(),
            ))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.into_iter().map(|(p, _, s)| (p, s)).collect()
}

fn f(x: &omr_scoring::Exact) -> f64 {
    x.to_f64().unwrap_or(0.0)
}

fn counts(p: &ExamProject) -> SeverityCounts {
    let mut c = SeverityCounts::default();
    for it in p.session.review.items.iter().filter(|i| i.is_open()) {
        match it.severity {
            Severity::Info => c.info += 1,
            Severity::Warn => c.warn += 1,
            Severity::Error => c.error += 1,
            Severity::Blocker => c.blocker += 1,
        }
    }
    // the key-sheet hold is lifted by confirming the key, not in the queue: one BLOCKER
    if p.session.key_unconfirmed() {
        c.blocker += 1;
    }
    // batch-scope findings are not queued; each counts at its own severity (an INFO note such
    // as MRK-016 `prior_model` is no warning, CR-18)
    for e in &p.session.batch_exceptions {
        match e.severity {
            Severity::Info => c.info += 1,
            Severity::Warn => c.warn += 1,
            Severity::Error => c.error += 1,
            Severity::Blocker => c.blocker += 1,
        }
    }
    c
}

fn key_label(p: &ExamProject) -> String {
    match &p.session.key {
        None => "No key".into(),
        Some(k) if p.session.key_unconfirmed() => {
            format!("Key v{} · from key sheets · NOT CONFIRMED", k.version)
        }
        Some(k) if k.inferred => format!("Key v{} · INFERRED", k.version),
        Some(k) if p.session.released() => format!("Key v{} · released", k.version),
        Some(k) => format!("Key v{} · in force", k.version),
    }
}

impl ExamProject {
    /// Dashboard row.
    pub fn summary_vm(&self) -> ExamSummary {
        let s = &self.session;
        let def = &s.definition;
        let mut sections: Vec<String> = def
            .roster
            .iter()
            .flatten()
            .map(|r| r.section.clone())
            .filter(|x| !x.is_empty())
            .collect();
        sections.sort();
        sections.dedup();
        let oc = counts(self);
        ExamSummary {
            id: def.exam_id.to_string(),
            course_code: def.course_code.clone(),
            course_title: def.course_title.clone(),
            course_title_ar: def.course_title_ar.clone(),
            assessment: def.title.clone(),
            term: def.term.clone(),
            date: def.date.clone(),
            versions: def.forms.clone(),
            status: if s.pages.is_empty() {
                ExamStatus::Setup
            } else if s.released() {
                ExamStatus::Released
            } else if s.release_blocked().is_some() {
                ExamStatus::InReview
            } else {
                ExamStatus::Ready
            },
            sheets_scanned: s.booklets.len() as u32,
            sheets_expected: def
                .roster
                .as_ref()
                .map_or(s.booklets.len() as u32, |r| r.len() as u32),
            open_exceptions: oc,
            key_status: key_label(self),
            mapped_items: def.outcomes.as_ref().map_or(0, |o| {
                let mut q: Vec<u16> = o.mapping.iter().map(|m| m.question).collect();
                q.sort_unstable();
                q.dedup();
                q.len() as u32
            }),
            total_items: u32::from(def.questions),
            roster_count: def.roster.as_ref().map_or(0, |r| r.len() as u32),
            sections,
            batch_id: self
                .store()
                .map_or(crate::store::BATCH_ID, |s| s.batch_id())
                .into(),
        }
    }

    /// Setup screen.
    pub fn setup_vm(&self) -> ExamSetup {
        let s = &self.session;
        let def = &s.definition;
        let legacy = matches!(def.template, crate::exam::TemplateChoice::Legacy(_));
        let template = TemplateInfo {
            id: def.template.id(),
            legacy,
            name: if legacy {
                "IAU CCSIT answer sheet".into()
            } else {
                "Generated A4 sheet".into()
            },
            questions: u32::from(def.questions),
            capacity: match &def.template {
                crate::exam::TemplateChoice::Legacy(_) => 100,
                crate::exam::TemplateChoice::Generated(t) => u32::from(t.questions),
            },
            option_labels: def.option_labels(),
            versions: def.forms.clone(),
            cancel_mode: format!("{:?}", def.cancel_mode),
            dpi: crate::engine::WORKING_DPI,
            page_identity: if legacy {
                "ring lattice + blank-form correlation (no fiducials)".into()
            } else {
                "QR v3-M page code + AprilTag 36h11".into()
            },
            digest: format!(
                "sha384 {}…{}",
                s.manifest.template_sha384.get(..4).unwrap_or(""),
                s.manifest
                    .template_sha384
                    .get(s.manifest.template_sha384.len().saturating_sub(4)..)
                    .unwrap_or("")
            ),
        };
        let keys = match &s.key {
            Some(k) => def
                .forms
                .iter()
                .enumerate()
                .map(|(fi, f)| VersionKey {
                    version: f.clone(),
                    answers: k.forms[if k.shared_items { 0 } else { fi }]
                        .iter()
                        .map(|c| c.text().replace('|', ","))
                        .collect(),
                })
                .collect(),
            None => Vec::new(),
        };
        let (clos, mapping, split) = match &def.outcomes {
            Some(o) => {
                let clos: Vec<CloDef> = o
                    .clos
                    .iter()
                    .map(|c| CloDef {
                        code: c.code.clone(),
                        title: c.title.clone(),
                        title_ar: c.title_ar.clone(),
                        domain: format!("{:?}", c.domain),
                        plo: c.plo.clone(),
                    })
                    .collect();
                let rows = (1..=def.questions)
                    .map(|q| MappingRow {
                        question: u32::from(q),
                        points: s
                            .key
                            .as_ref()
                            .and_then(|k| k.points.get(usize::from(q) - 1))
                            .and_then(|p| omr_scoring::parse_points(p).ok())
                            .and_then(|p| p.to_f64())
                            .unwrap_or(1.0),
                        cells: o
                            .clos
                            .iter()
                            .map(|c| {
                                // A cell stands for every version's row of (q, CLO): rows for
                                // one version only are labelled with their versions.
                                let rows: Vec<&crate::exam::CloMapping> = o
                                    .mapping
                                    .iter()
                                    .filter(|m| m.question == q && m.clo == c.code)
                                    .collect();
                                rows.first().map_or((None, String::new()), |m| {
                                    let note = if rows.iter().any(|m| !m.form.is_empty()) {
                                        let forms: Vec<&str> = rows
                                            .iter()
                                            .map(|m| {
                                                if m.form.is_empty() {
                                                    "every version"
                                                } else {
                                                    m.form.as_str()
                                                }
                                            })
                                            .collect();
                                        format!("version {} only: {}", forms.join(", "), m.note)
                                    } else {
                                        m.note.clone()
                                    };
                                    (
                                        omr_scoring::parse_points(&m.weight)
                                            .ok()
                                            .and_then(|w| w.to_f64()),
                                        note,
                                    )
                                })
                            })
                            .collect(),
                    })
                    .collect();
                (clos, rows, o.split)
            }
            None => (Vec::new(), Vec::new(), false),
        };
        let mut lints: Vec<(String, Severity, String)> = s
            .exam_exceptions
            .iter()
            .map(|e| (e.code.clone(), e.severity, e.detail.clone()))
            .collect();
        if let Some(sc) = &self.scoring {
            match &sc.run {
                Ok(r) => lints.extend(
                    r.exceptions
                        .iter()
                        .filter(|e| e.scope == omr_model::Scope::Exam)
                        .map(|e| (e.code.clone(), e.severity, e.detail.clone())),
                ),
                Err(e) => lints.push(("SCO-003".into(), Severity::Blocker, e.clone())),
            }
        }
        ExamSetup {
            exam: def.exam_id.to_string(),
            template,
            keys,
            key_status: key_label(self),
            key_published: s.key.as_ref().is_some_and(|k| !k.inferred) && !s.key_unconfirmed(),
            clos,
            mapping_split: split,
            mapping,
            lints,
        }
    }

    fn queue_item(&self, it: &ReviewItem) -> QueueItem {
        let s = &self.session;
        let b = it.booklet.map(|b| &s.booklets[b]);
        let form = b
            .and_then(|b| b.form)
            .map(|f| s.definition.forms[f].clone())
            .unwrap_or_default();
        QueueItem {
            id: it.id,
            label: match (it.kind, it.question) {
                (ItemKind::Mark, Some(q)) => format!("Q{}", q + 1),
                _ => b.map_or_else(
                    || {
                        it.page
                            .map(|p| format!("Page {}", s.pages[p].page_id))
                            .unwrap_or_else(|| "Exam".into())
                    },
                    |b| b.label(),
                ),
            },
            detail: match it.kind {
                ItemKind::Mark => format!(
                    "{} · {}",
                    if form.is_empty() {
                        "Form ?".into()
                    } else {
                        format!("Form {form}")
                    },
                    it.sub_reason.clone().unwrap_or_else(|| it.title.clone())
                ),
                _ => it.sub_reason.clone().unwrap_or_else(|| it.title.clone()),
            },
            question: it.question.map_or(0, |q| u32::from(q) + 1),
            form,
            serial: b.map_or_else(String::new, |b| format!("{:04}", b.serial)),
            severity: it.severity,
            fills: it.fills.clone(),
            kind: it.kind,
            deferred: it.state == ItemState::Deferred,
        }
    }

    /// The review queue of one lane.
    pub fn review_queue_vm(&self, lane: Lane) -> ReviewQueue {
        let s = &self.session;
        let lanes = Lane::ALL
            .iter()
            .map(|&l| (l, self.queue(Some(l)).len() as u32))
            .collect();
        let mut groups: Vec<ExceptionGroup> = Vec::new();
        for it in self.queue(Some(lane)) {
            let qi = self.queue_item(it);
            match groups.last_mut() {
                Some(g) if g.code == it.code && g.severity == it.severity => g.items.push(qi),
                _ => groups.push(ExceptionGroup {
                    code: it.code.clone(),
                    title: it.title.clone(),
                    severity: it.severity,
                    items: vec![qi],
                }),
            }
        }
        ReviewQueue {
            lane,
            lanes,
            groups,
            resolved: s.review.items.iter().filter(|i| !i.is_open()).count() as u32,
            total: s.review.items.len() as u32,
            audit_head: self
                .store()
                .and_then(|st| st.head())
                .map(|(seq, h)| format!("#{seq} {}…", &h[..12.min(h.len())]))
                .unwrap_or_default(),
        }
    }

    /// One review item with its crop (rendered from the stored original).
    pub fn review_item_vm(&self, id: u64) -> Option<ReviewItemView> {
        let s = &self.session;
        let it = s.review.item(id)?;
        let def = &s.definition;
        let labels = def.option_labels();
        let b = it.booklet.map(|b| &s.booklets[b]);
        let crop = self.crop(id).ok();
        let (cimg, bubbles) = match &crop {
            Some(c) => (
                CropImage {
                    width: c.width,
                    height: c.height,
                    rgba: c.rgba.clone(),
                },
                c.bubbles
                    .iter()
                    .map(|bb| BubbleView {
                        label: bb.label.clone(),
                        cx: bb.cx,
                        cy: bb.cy,
                        r: bb.r,
                        fill: bb.fill,
                        state: bb.state,
                    })
                    .collect(),
            ),
            None => (
                CropImage {
                    width: 1,
                    height: 1,
                    rgba: Arc::from(vec![255u8, 255, 255, 255]),
                },
                Vec::new(),
            ),
        };
        let other_fills: Vec<f32> = b
            .map(|b| {
                b.questions
                    .iter()
                    .enumerate()
                    .filter(|(q, qs)| {
                        Some(*q as u16) != it.question
                            && matches!(qs.current, omr_model::Response::Single(_))
                    })
                    .filter_map(|(q, qs)| {
                        let r = s.pages[qs.page].read.as_ref()?;
                        let k = r
                            .questions
                            .iter()
                            .position(|x| usize::from(x.question) == q)?;
                        r.question_geom
                            .get(k)
                            .map(|g| g.bubbles.iter().map(|x| x.fill).fold(0.0, f32::max))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut sorted = other_fills.clone();
        sorted.sort_by(f32::total_cmp);
        let typical = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        let members: Vec<u64> = s.review.group_members(it).iter().map(|m| m.id).collect();
        let unreadable = it.crop.as_ref().is_some_and(|c| {
            c.bubbles
                .iter()
                .any(|x| x.state == OverlayState::Unreadable)
        });
        let group = GroupConfirm {
            size: members.len() as u32,
            allowed: members.len() > 1 && !unreadable && it.severity != Severity::Blocker,
            reason: if unreadable {
                "An UNREADABLE bubble: decide items one by one".into()
            } else if it.severity == Severity::Blocker {
                "BLOCKER items are never batch-confirmed".into()
            } else if members.len() <= 1 {
                "No other open item in this group".into()
            } else {
                String::new()
            },
            members,
        };
        let page = it.page.map(|p| &s.pages[p]);
        let summary = match it.kind {
            ItemKind::Mark => format!(
                "Question {}, form {}. {}. Suggestion {}, engine.",
                it.question.map_or(0, |q| q + 1),
                b.and_then(|b| b.form)
                    .map_or("?".to_string(), |f| def.forms[f].clone()),
                it.fills
                    .iter()
                    .enumerate()
                    .map(|(o, x)| format!(
                        "{} {:.2}",
                        labels.get(o).cloned().unwrap_or_default(),
                        x
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                it.suggestion
                    .as_ref()
                    .map_or("none".into(), |d| d.text(&labels, &def.forms))
            ),
            _ => format!("{} {}: {}", it.code, it.title, it.detail),
        };
        let identity = (it.kind == ItemKind::Identity).then(|| {
            let b = b.expect("identity items have a booklet");
            IdentityView {
                read_id: b.identity.read.clone(),
                candidates: b
                    .identity
                    .candidates
                    .iter()
                    .map(|(k, p)| (k.clone(), *p as f32))
                    .collect(),
            }
        });
        let notice = matches!(it.kind, ItemKind::Notice | ItemKind::Form).then(|| {
            let mut lines = vec![("Detail".to_string(), it.detail.clone())];
            if let Some(p) = page {
                lines.push((
                    "Source".into(),
                    format!("{} p.{}", s.sources[p.source].name, p.index + 1),
                ));
                lines.push(("Status".into(), format!("{:?}", p.status)));
            }
            if let (ItemKind::Form, Some(b)) = (it.kind, b)
                && let Some(inf) = &b.inference
            {
                for (fi, ll) in inf.ll_milli.iter().enumerate() {
                    lines.push((
                        format!("LL form {}", def.forms[fi]),
                        format!("{:.1} nats", *ll as f64 / 1000.0),
                    ));
                }
                lines.push(("Declared".into(), b.declared_text.clone()));
            }
            let consequence = match (it.kind, it.lane) {
                (ItemKind::Form, _) => "The booklet is scored under the chosen form".to_string(),
                (_, Lane::Capture) => "The page leaves the batch (rescan pull list)".to_string(),
                _ => "Recorded; the engine's handling stands".to_string(),
            };
            NoticeView {
                headline: it.title.clone(),
                lines,
                consequence,
            }
        });
        let read = page.and_then(|p| p.read.as_ref());
        Some(ReviewItemView {
            id,
            code: it.code.clone(),
            code_title: it.title.clone(),
            sub_reason: it.sub_reason.clone().unwrap_or_default(),
            severity: it.severity,
            kind: it.kind,
            question: it.question.map_or(0, |q| u32::from(q) + 1),
            form: b
                .and_then(|b| b.form)
                .map(|f| def.forms[f].clone())
                .unwrap_or_default(),
            serial: b.map_or_else(String::new, |b| format!("{:04}", b.serial)),
            page: page.map_or(0, |p| p.index + 1),
            dpi: crate::engine::WORKING_DPI,
            crop: cimg,
            bubbles,
            threshold: it
                .crop
                .as_ref()
                .map_or(read.map_or(0.2, |r| f32::from(r.theta_sel) / 1024.0), |c| {
                    c.threshold
                }),
            band: it.crop.as_ref().map_or((0.18, 0.22), |c| c.band),
            suggestion: it.suggestion.clone(),
            answered: other_fills.len() as u32,
            other_fills,
            typical_full_fill: typical,
            summary,
            group,
            current: it.resolution.clone(),
            identity,
            notice,
        })
    }

    /// Review events, oldest first.
    pub fn history_vm(&self) -> Vec<DecisionRecord> {
        let s = &self.session;
        let labels = s.definition.option_labels();
        s.review
            .history
            .iter()
            .map(|r| {
                let it = s.review.item(r.item);
                DecisionRecord {
                    seq: r.seq,
                    item: r.item,
                    question: it.and_then(|i| i.question).map_or(0, |q| u32::from(q) + 1),
                    serial: it
                        .and_then(|i| i.booklet)
                        .map_or_else(String::new, |b| format!("{:04}", s.booklets[b].serial)),
                    action: match &r.action {
                        crate::review::Action::Resolve { decision, .. } => {
                            format!("resolve {}", decision.text(&labels, &s.definition.forms))
                        }
                        crate::review::Action::ApplyToGroup { decision } => {
                            format!("group {}", decision.text(&labels, &s.definition.forms))
                        }
                        a => format!("{a:?}").to_lowercase(),
                    } + &r
                        .reason
                        .as_ref()
                        .map(|r| format!(" ({})", r.code))
                        .unwrap_or_default(),
                    undone: r.undone,
                }
            })
            .collect()
    }

    /// Results screen.
    pub fn results_vm(&self) -> Option<Results> {
        let sc = self.scoring.as_ref()?;
        let run = sc.run.as_ref().ok()?;
        let grades = sc.grades();
        let pcts: Vec<f64> = run
            .students
            .iter()
            .filter_map(|r| r.final_pct.as_ref().map(f))
            .collect();
        let n = pcts.len();
        let mean = if n == 0 {
            0.0
        } else {
            pcts.iter().sum::<f64>() / n as f64
        };
        let sd = if n == 0 {
            0.0
        } else {
            (pcts.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt()
        };
        let mut sorted = pcts.clone();
        sorted.sort_by(f64::total_cmp);
        let median = if n == 0 {
            0.0
        } else if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        let mut histogram = vec![0u32; 10];
        for x in &pcts {
            histogram[((x / 10.0).floor() as usize).min(9)] += 1;
        }
        let scale = omr_scoring::GradeScale::saudi();
        let passing = grades.iter().flatten().filter(|g| g.is_passing()).count();
        let bands = scale
            .bands
            .iter()
            .map(|band| GradeBand {
                letter: band.letter.clone(),
                letter_ar: band.arabic.clone(),
                lo: band.lo.as_ref().map_or(0.0, f),
                hi: band.hi.as_ref().map(f),
                count: grades
                    .iter()
                    .flatten()
                    .filter(|g| g.code() == band.letter)
                    .count() as u32,
                fail: band.letter == "F",
            })
            .collect();
        let analytics = self.analytics();
        let items = analytics
            .as_ref()
            .map(|a| {
                a.items
                    .iter()
                    .map(|i| ItemStats {
                        // Master items are per form when the forms' keys differ (`B-Q3`):
                        // the printed question is the position within the form's block.
                        question: (i.item % usize::from(self.session.definition.questions).max(1))
                            as u32
                            + 1,
                        label: i.label.clone(),
                        p: i.p.unwrap_or(0.0),
                        r_ir: i.r_ir.unwrap_or(0.0),
                        r_ci: i.r_ir_ci.unwrap_or((0.0, 0.0)),
                        option_shares: i
                            .options
                            .iter()
                            .filter(|o| o.option.is_some())
                            .map(|o| o.proportion)
                            .collect(),
                        flags: a
                            .flags
                            .iter()
                            .filter(|fl| fl.item == i.item)
                            .map(|fl| (fl.message.clone(), None))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let test = analytics.as_ref().and_then(|a| a.tests.first());
        let s = &self.session;
        let mut readiness = vec![
            (
                s.open_blocking() == 0,
                format!("{} ERROR/BLOCKER review items open", s.open_blocking()),
            ),
            (
                !s.key.as_ref().is_some_and(|k| k.inferred),
                if s.key.as_ref().is_some_and(|k| k.inferred) {
                    "Key INFERRED from the sheets: not releasable".into()
                } else {
                    "Answer key supplied".into()
                },
            ),
        ];
        let provisional = run
            .students
            .iter()
            .filter(|r| r.status == omr_scoring::AssessmentStatus::Provisional)
            .count();
        readiness.push((
            provisional == 0,
            format!("{provisional} provisional totals"),
        ));
        // §12.2: the probable mis-keys of the current results are judged and queued.
        let miskeys = s
            .review
            .items
            .iter()
            .filter(|i| i.is_open() && i.code == "SCO-001")
            .count();
        readiness.push((
            !s.flags.computing() && miskeys == 0,
            if s.flags.computing() {
                "Mis-key check (§12.2 flag stability): computing…".into()
            } else {
                format!("{miskeys} probable mis-keys (SCO-001) to decide")
            },
        ));
        let unattributed = s
            .pages
            .iter()
            .filter(|p| matches!(p.status, PageStatus::Unattributed | PageStatus::Refused))
            .count();
        readiness.push((
            unattributed == 0,
            format!("{unattributed} pages not attributed"),
        ));
        let inferred_forms = s
            .booklets
            .iter()
            .filter(|b| b.form_source == FormSource::Inferred)
            .count();
        readiness.push((
            inferred_forms == 0,
            format!("{inferred_forms} forms inferred"),
        ));
        let pending = s
            .booklets
            .iter()
            .flat_map(|b| &b.questions)
            .filter(|q| q.status == ResponseStatus::Pending)
            .count();
        readiness.push((pending == 0, format!("{pending} responses pending review")));
        Some(Results {
            key_label: key_label(self),
            students: n as u32,
            mean_pct: mean,
            median_pct: median,
            sd_pct: sd,
            pass_rate: if n == 0 {
                0.0
            } else {
                passing as f64 / n as f64
            },
            alpha: test.and_then(|t| t.alpha.map(|a| (a, t.alpha_ci.unwrap_or((a, a))))),
            histogram,
            grades: bands,
            items,
            option_labels: s.definition.option_labels(),
            readiness,
            release_blocked: s.release_blocked(),
            absent: self.absent_students().len() as u32,
        })
    }

    /// Outcomes screen.
    pub fn outcomes_vm(&self) -> Option<Outcomes> {
        let o = self.outcomes()?;
        let clos = o
            .result
            .clos
            .iter()
            .map(|c| CloRow {
                code: c.clo.clone(),
                title: o
                    .clos
                    .clo(&c.clo)
                    .map(|x| x.text_en.clone())
                    .unwrap_or_default(),
                target: f(&c.target),
                average: c.stats.mean.as_ref().map(f),
                proportion: c.stats.proportion.as_ref().map(f),
                n: c.stats.n as u32,
                attained: c.attained,
                indistinguishable: c.indistinguishable,
            })
            .collect();
        let tp154_b1 =
            o.b1.groups
                .iter()
                .flat_map(|g| {
                    g.rows.iter().map(|r| {
                        [
                            r.number.clone(),
                            r.clo_text.clone(),
                            r.plo_codes.join(", "),
                            r.methods.clone(),
                            r.targeted.clone(),
                            r.actual.clone(),
                            r.comment.clone(),
                        ]
                    })
                })
                .collect();
        Some(Outcomes {
            method: o.policy.class_method.name().into(),
            tau: f(&o.policy.tau),
            target: f(&o.policy.target),
            clos,
            tp154_b1,
            classification: "Restricted · مقيد".into(),
        })
    }

    /// One booklet's responses as text (CLI and tests).
    pub fn booklet_responses(&self, bi: usize) -> Vec<String> {
        let labels = self.session.definition.option_labels();
        self.session.booklets[bi]
            .questions
            .iter()
            .map(|q| response_text(&q.current, &labels))
            .collect()
    }
}
