//! [`AppBackend`]: the [`Backend`] over the real application core (spec §4.7). A thin adapter:
//! every value comes from an `omr_app::ExamProject`, its view-models (`omr_app::vm`) and, where a
//! view-model lacks a field the shell shows, the project's public session state. Nothing is
//! invented: what the core does not record (review times, the handwriting and seat-map identity
//! channels, a separate unregistered crop, L6 status counts, a two-person rule) stays empty, and
//! the shell shows an honest empty state or leaves the element out.
//!
//! Projects open read-write on first use (one store writer each). A project whose store cannot be
//! opened for writing (another process holds it, e.g. `omr review`) opens read-only, and every
//! command says how to free it. Grading runs on a worker thread that the binary starts with
//! [`run_grading`] (rule R4: this library creates no threads and does no file I/O itself; omr-app
//! does both); [`Backend::pump`] drains the worker's [`JobEvent`]s without blocking. Adding a
//! later batch to an open exam runs [`run_append`] on a thread the binary's [`Spawn`] starts:
//! the project is saved and closed on the UI thread, regraded with the new scans on the worker
//! and reopened when the worker reports.

use crate::backend::{self as b, Backend, BackendError, ExamId, ItemId};
use omr_app::exam::{DomainLetter, ExamDefinition};
use omr_app::model::{PageStatus, ResponseStatus};
use omr_app::review::{
    Action, Decision as AppDecision, ItemKind, Lane as AppLane, OverlayState, ReviewReason,
};
use omr_app::vm::{self, ProgressTracker};
use omr_app::{AppError, ExamProject, GradeOptions, Progress};
use omr_model::Severity as AppSeverity;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

/// Thumbnails shown on the Scan screen (the grid shows the rows that fit).
const THUMBS: usize = 30;
/// An identity candidate below this posterior is never the suggested (Enter) decision.
const MIN_IDENTITY_SUGGESTION: f32 = 0.5;
/// Width of a page thumbnail in pixels (the tile is about 100 px wide at 1440 px).
const THUMB_WIDTH: usize = 120;

// ------------------------------------------------------------------------------------ grading

/// What a grading worker reports.
#[derive(Debug, Clone)]
pub enum JobEvent {
    /// A pipeline progress event (§4.5).
    Progress(Progress),
    /// A line for the batch activity log (e.g. what an added batch changed).
    Note(String),
    /// The run ended: the project package (`project.omrproj`), or the error with remediation.
    Finished(Result<PathBuf, String>),
}

/// Starts `work` on a thread the binary owns (rule R4: the library creates none); an error says
/// why no thread could be started.
pub type Spawn = Box<dyn Fn(Box<dyn FnOnce() + Send>) -> Result<(), String>>;

/// A grading run to start: the definition, the inputs and the pipeline options (whose
/// `project_dir` must name the new package).
#[derive(Debug, Clone)]
pub struct GradeRequest {
    pub definition: ExamDefinition,
    pub inputs: Vec<PathBuf>,
    /// Files named as key sheets (§10.1): graded with the exam, held until confirmed.
    pub key_sheets: Vec<PathBuf>,
    pub options: GradeOptions,
}

/// The body of a grading worker thread: grades, saves and closes the project, and reports every
/// progress event and the outcome on `tx`. Never panics on bad input: errors arrive as
/// [`JobEvent::Finished`]`(Err(..))` with the core's remediation text.
pub fn run_grading(req: GradeRequest, tx: Sender<JobEvent>) {
    let progress_tx = tx.clone();
    let progress = move |p: &Progress| {
        // the window may be gone; the run still finishes and saves
        let _ = progress_tx.send(JobEvent::Progress(p.clone()));
    };
    let outcome = match &req.options.project_dir {
        None => Err("internal: a desktop grading run needs a project folder".to_string()),
        Some(dir) => ExamProject::grade_with_key_sheets(
            &req.definition,
            &req.inputs,
            &req.key_sheets,
            &req.options,
            &progress,
        )
        .and_then(|p| p.close())
        .map(|()| dir.clone())
        .map_err(|e| {
            format!(
                "{e} (exit code {}). Fix the input and start again; `omr grade` prints \
                         the same message",
                e.exit_code()
            )
        }),
    };
    let _ = tx.send(JobEvent::Finished(outcome));
}

/// A later batch to add to a graded exam: its project package, the new scan files or folders
/// and the pipeline options (`threads`, `operator`; the exam's snapshot time is kept).
#[derive(Debug, Clone)]
pub struct AppendRequest {
    pub project: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub options: GradeOptions,
}

/// The body of an append worker thread: opens the project, grades it again with the new scans
/// ([`ExamProject::add_scans`]), saves and closes it, and reports progress, a summary line and
/// the outcome on `tx`. The project stays usable when the batch is refused: the shell reopens
/// it either way.
pub fn run_append(req: AppendRequest, tx: Sender<JobEvent>) {
    let progress_tx = tx.clone();
    let progress = move |p: &Progress| {
        let _ = progress_tx.send(JobEvent::Progress(p.clone()));
    };
    let fail = |e: AppError| {
        format!(
            "{e} (exit code {}). Nothing was added; `omr add --project {}` prints the same message",
            e.exit_code(),
            req.project.display()
        )
    };
    let outcome = match ExamProject::open(&req.project, &req.options.operator) {
        Err(e) => Err(fail(e)),
        Ok(mut p) => {
            let added = p.add_scans(&req.inputs, &req.options, &progress);
            let closed = p.close();
            match (added, closed) {
                (Ok(r), Ok(())) => {
                    let _ = tx.send(JobEvent::Note(append_summary(&r)));
                    Ok(req.project.clone())
                }
                (Err(e), _) | (Ok(_), Err(e)) => Err(fail(e)),
            }
        }
    };
    let _ = tx.send(JobEvent::Finished(outcome));
}

/// "batch-2 · 4 files · 4 new pages · 60 → 64 booklets · 3 decisions kept · 0 conflicts".
fn append_summary(r: &omr_app::append::AppendReport) -> String {
    let mut parts = vec![
        r.batch.clone(),
        plural(r.files, "file"),
        format!(
            "{} new page{}",
            r.new_pages,
            if r.new_pages == 1 { "" } else { "s" }
        ),
        format!("{} → {} booklets", r.booklets_before, r.booklets_after),
        format!(
            "{} decision{} kept",
            r.carried_over,
            if r.carried_over == 1 { "" } else { "s" }
        ),
    ];
    if r.duplicates > 0 {
        parts.push(format!("{} already imported (CAP-011)", r.duplicates));
    }
    if r.conflicts > 0 {
        parts.push(format!("{} reread differently (SYS-032)", r.conflicts));
    }
    parts.join(" · ")
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// What a background run is doing.
enum JobKind {
    /// A new exam graded into a new project.
    Grade { definition: Box<ExamDefinition> },
    /// A later batch added to an existing project, reopened from `project` when done.
    Append { project: PathBuf, batch: u32 },
}

struct Job {
    exam: ExamId,
    kind: JobKind,
    rx: Receiver<JobEvent>,
    tracker: ProgressTracker,
    /// Newest first.
    activity: Vec<String>,
    outcome: Option<Result<PathBuf, String>>,
    /// Live sheet states by page ID, classified as the tracker does (auto, review, rejected).
    sheets: BTreeMap<String, u8>,
}

impl Job {
    fn running(&self) -> bool {
        self.outcome.is_none()
    }

    fn is_append(&self) -> bool {
        matches!(self.kind, JobKind::Append { .. })
    }

    fn sheet_counts(&self) -> b::SheetCounts {
        let mut c = b::SheetCounts::default();
        for st in self.sheets.values() {
            match st {
                0 => c.auto += 1,
                1 => c.review += 1,
                _ => c.rejected += 1,
            }
        }
        c
    }
}

// ------------------------------------------------------------------------------------ projects

/// A dashboard row: where the project lives and what the workspace scan saw.
struct Entry {
    id: ExamId,
    dir: PathBuf,
    summary: vm::ExamSummary,
    snapshot_utc: String,
    pages: usize,
    page_stage_ms: f64,
}

/// Page thumbnails by project folder and page ID, decoded from the stored originals at first
/// sight. A stored page never changes, and the cache outlives the project's reopening after an
/// added batch, so the pages already shown are not decoded again (about 28 ms each).
type Thumbs = RefCell<BTreeMap<(PathBuf, String), Option<b::CropImage>>>;

/// An opened project and the derived view-models cached until its next change.
struct Opened {
    p: ExamProject,
    /// `Some(reason)` when the store could not be opened for writing.
    read_only: Option<String>,
    dir: PathBuf,
    items: RefCell<BTreeMap<u64, Option<b::ReviewItem>>>,
    results: RefCell<Option<Option<b::Results>>>,
    outcomes: RefCell<Option<Option<b::Outcomes>>>,
    /// The backend's page thumbnails (see [`Thumbs`]).
    thumbs: Rc<Thumbs>,
    /// Key-sheet files read for this exam and not confirmed yet (nothing in the exam changes
    /// until the confirmation replaces its key).
    preview: Option<Preview>,
    /// The key-sheet summary (with its row crops), cached until the next change.
    keysheets: RefCell<Option<Option<b::KeySheetSummary>>>,
    /// Flag stability (§12.2) of a score run: its run ID and the stability job's report.
    stability: RefCell<Option<(String, omr_analytics::StabilityReport)>>,
    /// A Remark import read in Setup and not applied yet (§3.5).
    remark: Option<HeldImport>,
    /// What the last parallel run with Remark read (§23.7): the report is written from a
    /// rerun on the exam as it stands.
    parallel: Option<b::RemarkRequest>,
}

/// Key-sheet files read in memory for an exam (`omr key read`): the recognizer's reading, held
/// until the instructor confirms it.
struct Preview {
    p: ExamProject,
    /// The files as given, for the summary and the audit note.
    files: Vec<String>,
    /// Their paths: at the confirmation they are added to the exam as named key sheets
    /// (`ExamProject::add_key_sheets`), so the originals are stored and the key version names
    /// them (C7).
    paths: Vec<PathBuf>,
}

impl Opened {
    fn new(p: ExamProject, dir: PathBuf, read_only: Option<String>, thumbs: Rc<Thumbs>) -> Self {
        Self {
            p,
            read_only,
            dir,
            items: RefCell::new(BTreeMap::new()),
            results: RefCell::new(None),
            outcomes: RefCell::new(None),
            thumbs,
            preview: None,
            keysheets: RefCell::new(None),
            stability: RefCell::new(None),
            remark: None,
            parallel: None,
        }
    }

    /// Drops every cached view-model (after a command changed the project).
    fn changed(&self) {
        self.items.borrow_mut().clear();
        *self.results.borrow_mut() = None;
        *self.outcomes.borrow_mut() = None;
        *self.keysheets.borrow_mut() = None;
    }

    fn writable(&self) -> Result<(), BackendError> {
        match &self.read_only {
            None => Ok(()),
            Some(why) => Err(BackendError::Denied(format!(
                "This exam is open read-only ({why}). Close the other program that has {} open \
                 (an `omr review` command or another window), then reopen the exam",
                self.dir.display()
            ))),
        }
    }
}

/// The [`Backend`] over `omr-app`; see the module docs.
pub struct AppBackend {
    operator: String,
    workspace: Option<PathBuf>,
    entries: Vec<Entry>,
    opened: RefCell<BTreeMap<ExamId, Opened>>,
    /// Projects that failed to open, with the reason (shown instead of their screens).
    failed: RefCell<BTreeMap<ExamId, String>>,
    job: Option<Job>,
    default: Option<ExamId>,
    /// Starts append workers (set by the binary); without it scans cannot be added.
    spawn: Option<Spawn>,
    /// Exams reloaded since [`Backend::take_reloaded`] was last called.
    reloaded: Vec<ExamId>,
    /// Flag-stability jobs running on the spawner's threads (§12.3: a background job, never on
    /// the synchronous regrade path): exam, score-run ID and the channel of its report.
    stability_jobs: RefCell<Vec<StabilityJob>>,
    /// A Remark import read on the New-exam form and not used yet (§3.5).
    remark_new: Option<HeldImport>,
    /// Page thumbnails of every project opened (shared with each [`Opened`]).
    thumbs: Rc<Thumbs>,
}

struct StabilityJob {
    exam: ExamId,
    run_id: String,
    rx: Receiver<omr_analytics::StabilityReport>,
}

fn app_err(e: AppError) -> BackendError {
    let code = e.exit_code();
    match e {
        AppError::NotFound(m) => BackendError::NotFound(format!(
            "{m}. The item may have been resolved elsewhere: reopen the exam"
        )),
        AppError::Review(m) | AppError::Definition(m) | AppError::Scoring(m) => {
            BackendError::Invalid(m)
        }
        AppError::Input(m) => BackendError::Invalid(format!("{m} (input error, code {code})")),
        AppError::Store(m) => BackendError::Denied(format!(
            "The project store refused the change: {m}. Check that the folder is writable and \
             not open elsewhere, then reopen the exam"
        )),
        AppError::Io(e) => BackendError::Denied(format!(
            "File error: {e}. Check that the project folder is writable and the disk is not full"
        )),
        AppError::Internal(m) => BackendError::Denied(format!(
            "Internal error: {m}. Nothing was changed; please report it with the project folder"
        )),
        AppError::Cancelled => BackendError::Denied(
            "Cancelled: nothing was saved; your scan files are unchanged".into(),
        ),
    }
}

impl AppBackend {
    fn empty(operator: &str, workspace: Option<PathBuf>) -> Self {
        Self {
            operator: operator.to_string(),
            workspace,
            entries: Vec::new(),
            opened: RefCell::new(BTreeMap::new()),
            failed: RefCell::new(BTreeMap::new()),
            job: None,
            default: None,
            spawn: None,
            reloaded: Vec::new(),
            stability_jobs: RefCell::new(Vec::new()),
            remark_new: None,
            thumbs: Rc::default(),
        }
    }

    /// Lets the backend start background work (adding scans) on threads the caller owns.
    pub fn set_spawner(&mut self, spawn: Spawn) {
        self.spawn = Some(spawn);
    }

    /// True while `exam`'s project is closed for an added batch.
    fn busy(&self, exam: &ExamId) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| j.is_append() && j.running() && &j.exam == exam)
    }

    /// Every graded exam below `workspace` (folders holding `project.omrproj`, via
    /// `omr_app::vm::list_exams`), newest first. Projects open read-write when first shown.
    pub fn workspace(workspace: &Path, operator: &str) -> Self {
        let mut me = Self::empty(operator, Some(workspace.to_path_buf()));
        for (dir, summary) in vm::list_exams(workspace) {
            me.add_entry(dir, summary, None);
        }
        me.default = me.entries.first().map(|e| e.id.clone());
        me
    }

    /// One graded project: `dir` is the exam folder or its `project.omrproj`.
    pub fn project(dir: &Path, operator: &str) -> Result<Self, String> {
        let dir = project_dir(dir);
        let mut me = Self::empty(operator, dir.parent().map(Path::to_path_buf));
        let (p, read_only) = open_project(&dir, operator)?;
        let summary = p.summary_vm();
        let id = me.add_entry(dir.clone(), summary, Some(&p));
        me.opened.borrow_mut().insert(
            id.clone(),
            Opened::new(p, dir, read_only, me.thumbs.clone()),
        );
        me.default = Some(id);
        Ok(me)
    }

    /// A grading run in progress: `rx` receives the worker's events (see [`run_grading`]).
    /// The exams of `workspace`, if given, are listed beside it.
    pub fn grading(
        definition: &ExamDefinition,
        rx: Receiver<JobEvent>,
        workspace: Option<&Path>,
        operator: &str,
    ) -> Self {
        let mut me = match workspace {
            Some(w) => Self::workspace(w, operator),
            None => Self::empty(operator, None),
        };
        let exam = me.start_grade_job(definition, rx);
        me.default = Some(exam);
        me
    }

    /// Shows a grading run of `definition` whose worker reports on `rx`; returns its exam ID.
    fn start_grade_job(&mut self, definition: &ExamDefinition, rx: Receiver<JobEvent>) -> ExamId {
        // the same definition graded again into another folder is another exam on the dashboard
        let mut exam = ExamId(definition.exam_id.to_string());
        let mut n = 2;
        while self.entries.iter().any(|e| e.id == exam) {
            exam = ExamId(format!("{}#{n}", definition.exam_id));
            n += 1;
        }
        self.job = Some(Job {
            exam: exam.clone(),
            kind: JobKind::Grade {
                definition: Box::new(definition.clone()),
            },
            rx,
            tracker: ProgressTracker::default(),
            activity: vec![format!(
                "{} · queued for grading",
                snapshot_label(&omr_app::pipeline::now_utc())
            )],
            outcome: None,
            sheets: BTreeMap::new(),
        });
        exam
    }

    /// Starts grading the New-exam form (see [`Backend::create_exam`]).
    fn start_new_exam(&mut self, form: &b::NewExam) -> Result<(ExamId, String), BackendError> {
        let Some(workspace) = self.workspace.clone() else {
            return Err(BackendError::Denied(
                "No exams folder is open: start omr-desktop with --workspace DIR (or without \
                 flags, for ~/OMR Exams)"
                    .into(),
            ));
        };
        if let Some(j) = self.job.as_ref().filter(|j| j.running()) {
            return Err(BackendError::Denied(format!(
                "{} is still being graded: create the next exam when the Scan screen says \
                 complete",
                j.exam
            )));
        }
        let Some(spawn) = self.spawn.as_ref() else {
            return Err(BackendError::Denied(
                "This window cannot start background work; grade with `omr grade <scans…> \
                 --out <folder in the exams folder>` and reopen the workspace"
                    .into(),
            ));
        };
        let bad = |m: String| BackendError::Invalid(m);
        let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let inputs = crate::bind::scan_paths(&form.scans);
        if inputs.is_empty() {
            return Err(bad(
                "Scans: type the folder or the scan files to grade (several: separate them \
                 with ;)"
                    .into(),
            ));
        }
        let questions: u16 = form
            .questions
            .trim()
            .parse()
            .ok()
            .filter(|q| *q > 0)
            .ok_or_else(|| {
                bad(format!(
                    "Questions: type how many questions this exam uses, e.g. 30 (not {:?})",
                    form.questions.trim()
                ))
            })?;
        let options = match text(&form.options) {
            None => None,
            Some(o) => Some(o.parse::<u8>().map_err(|_| {
                bad(format!(
                    "Options: type the options per question, e.g. 4 or 5 (not {o:?}), or leave \
                     it blank for the sheet's"
                ))
            })?),
        };
        // one path, quotes and a leading ~/ handled as for the scans
        let path = |s: &str| crate::bind::scan_paths(s).into_iter().next();
        let sheet = text(&form.sheet).unwrap_or_else(|| "iau-ccsit-v1".into());
        let sheet = match omr_app::define::Sheet::parse(&sheet) {
            omr_app::define::Sheet::Spec(p) => {
                omr_app::define::Sheet::Spec(path(&sheet).unwrap_or(p))
            }
            t => t,
        };
        let mut i = omr_app::define::Inputs::new(sheet, questions);
        i.options = options;
        i.forms = text(&form.forms).map(|f| {
            f.split([',', ' '])
                .map(|x| x.trim().to_uppercase())
                .filter(|x| !x.is_empty())
                .collect()
        });
        i.key = path(&form.key);
        i.infer_key = i.key.is_none();
        // key sheets named in the form: read with the exam, held until confirmed in Setup
        let key_sheets = crate::bind::scan_paths(&form.key_sheets);
        i.roster = path(&form.roster);
        i.clo = path(&form.clo);
        i.title = text(&form.title);
        i.course = text(&form.course);
        // the key, its exact points per version and the learning objectives from a Remark
        // import read on this form (§3.5): applied to the definition as omr-app gives them
        let remark = match text(&form.remark) {
            None => None,
            Some(named) => Some(self.remark_for_new_exam(&named, &mut i, &key_sheets)?),
        };
        let (mut definition, warnings) = omr_app::define::define(&i).map_err(app_err)?;
        if let Some(imp) = &remark {
            definition.key = omr_app::exam::KeySource::Provided {
                forms: imp.form_keys(),
            };
            definition.set_key_points(&imp.key_points());
            if let Some(o) = imp.outcome_setup() {
                definition.outcomes = Some(o);
            }
            definition.exam_id = definition.derived_exam_id();
            definition.validate().map_err(app_err)?;
        }
        // the inputs exist and hold scans before anything is created
        let files = omr_app::ingest::collect_paths(&inputs).map_err(app_err)?;
        let dir = omr_app::define::new_exam_dir(&workspace, &definition);
        let req = GradeRequest {
            definition: definition.clone(),
            inputs,
            key_sheets: key_sheets.clone(),
            options: GradeOptions {
                threads: 0,
                project_dir: Some(dir.join("project.omrproj")),
                snapshot_utc: None,
                operator: self.operator.clone(),
                max_pages: None,
                cancel: Default::default(),
            },
        };
        let (tx, rx) = std::sync::mpsc::channel::<JobEvent>();
        spawn(Box::new(move || run_grading(req, tx))).map_err(|e| {
            BackendError::Denied(format!(
                "Cannot start the grading worker ({e}); close other programs and try again"
            ))
        })?;
        let exam = self.start_grade_job(&definition, rx);
        if let Some(job) = self.job.as_mut() {
            let now = snapshot_label(&omr_app::pipeline::now_utc());
            for w in warnings {
                job.activity.insert(0, format!("{now} · roster: {w}"));
            }
        }
        if let (Some(imp), Some(job)) = (&remark, self.job.as_mut()) {
            let now = snapshot_label(&omr_app::pipeline::now_utc());
            for line in remark_activity(imp) {
                job.activity.insert(0, format!("{now} · {line}"));
            }
        }
        self.remark_new = None;
        self.default = Some(exam.clone());
        let key = if remark.is_some() {
            "; the key and its points per version come from the Remark import, the learning \
             objectives as CLO mappings flagged imported, needs review"
        } else if !key_sheets.is_empty() {
            "; the key is read from the key sheets and waits for your confirmation in Setup \
             (nothing is scored until then)"
        } else if i.infer_key {
            "; no key file, so the key is inferred and the results stay provisional until you \
             replace it in Setup"
        } else {
            ""
        };
        Ok((
            exam,
            format!(
                "Grading {} into {}{key}",
                plural(files.len(), "scan file"),
                dir.display()
            ),
        ))
    }

    /// The Remark import held for the New-exam form, checked against what the form says
    /// (`named`: the workbooks it lists) and the form's other fields; sets the questions and
    /// versions it gives and that no key is inferred.
    fn remark_for_new_exam(
        &self,
        named: &str,
        i: &mut omr_app::define::Inputs,
        key_sheets: &[PathBuf],
    ) -> Result<omr_app::remark::RemarkImport, BackendError> {
        let bad = |m: String| BackendError::Invalid(m);
        let held = self.remark_new.as_ref().ok_or_else(|| {
            bad(
                "Remark import: the export read on this form is no longer held; read it again \
                 with Import from Remark export…"
                    .into(),
            )
        })?;
        let files: Vec<String> = held.files.iter().map(|f| file_label(f)).collect();
        if files.join("; ") != named.trim() {
            return Err(bad(format!(
                "Remark import: the form lists {named} but the export read is {}; read it again \
                 with Import from Remark export…",
                files.join("; ")
            )));
        }
        let imp = &held.imp;
        if i.key.is_some() {
            return Err(bad(
                "Answer key file: the key comes from the Remark import; clear the key file, or \
                 clear the import"
                    .into(),
            ));
        }
        if !key_sheets.is_empty() {
            return Err(bad(
                "Key sheets: the key comes from the Remark import; clear the key sheets, or \
                 clear the import"
                    .into(),
            ));
        }
        if i.questions != imp.questions {
            return Err(bad(format!(
                "Number of questions: the Remark export has {} questions (not {}); type {} or \
                 clear the import",
                imp.questions, i.questions, imp.questions
            )));
        }
        let forms = imp.forms();
        if let Some(f) = i.forms.as_ref().filter(|f| !f.is_empty() && **f != forms) {
            return Err(bad(format!(
                "Form letters: the Remark export gives versions {} (not {}); type {} or clear \
                 the import",
                forms.join(","),
                f.join(","),
                forms.join(",")
            )));
        }
        let objectives = imp.versions.iter().any(|v| !v.groups.is_empty());
        if objectives && i.clo.is_some() {
            return Err(bad(
                "CLO mapping file: the learning objectives come from the Remark import; clear \
                 the CLO file, or clear the import"
                    .into(),
            ));
        }
        i.forms = Some(forms);
        i.infer_key = false;
        Ok(imp.clone())
    }

    /// True while a grading run or an added batch is in progress (screenshots wait for it).
    pub fn grading_in_progress(&self) -> bool {
        self.job.as_ref().is_some_and(Job::running)
    }

    /// The error of a failed grading run, if any.
    pub fn grading_error(&self) -> Option<String> {
        match self.job.as_ref()?.outcome.as_ref()? {
            Err(e) => Some(e.clone()),
            Ok(_) => None,
        }
    }

    /// Runs `f` on the opened project of `exam` (tests use it as an independent oracle).
    pub fn with_project<R>(&self, exam: &ExamId, f: impl FnOnce(&ExamProject) -> R) -> Option<R> {
        self.with(exam, |o| f(&o.p))
    }

    /// Adds a dashboard row; `opened` is the project when it is already open (else a read-only
    /// open reads the snapshot time and timings the dashboard needs).
    fn add_entry(
        &mut self,
        dir: PathBuf,
        summary: vm::ExamSummary,
        opened: Option<&ExamProject>,
    ) -> ExamId {
        let mut id = ExamId(summary.id.clone());
        let mut n = 2;
        while self.entries.iter().any(|e| e.id == id) {
            id = ExamId(format!("{}#{n}", summary.id));
            n += 1;
        }
        let details = |p: &ExamProject| {
            (
                p.session.snapshot_utc.clone(),
                p.session.timings.pages,
                p.session.timings.page_stage_ms,
            )
        };
        let (snapshot_utc, pages, page_stage_ms) = match opened {
            Some(p) => details(p),
            None => ExamProject::open_read_only(&dir)
                .map(|p| details(&p))
                .unwrap_or_default(),
        };
        self.entries.push(Entry {
            id: id.clone(),
            dir,
            summary,
            snapshot_utc,
            pages,
            page_stage_ms,
        });
        id
    }

    /// Opens `exam` on first use; `None` when it is unknown, failed to open or is closed for an
    /// added batch (it reopens when the batch is done).
    fn with<R>(&self, exam: &ExamId, f: impl FnOnce(&Opened) -> R) -> Option<R> {
        if self.busy(exam) {
            return None;
        }
        if !self.opened.borrow().contains_key(exam) && !self.failed.borrow().contains_key(exam) {
            let dir = self.entries.iter().find(|e| &e.id == exam)?.dir.clone();
            match open_project(&dir, &self.operator) {
                Ok((p, ro)) => {
                    self.opened
                        .borrow_mut()
                        .insert(exam.clone(), Opened::new(p, dir, ro, self.thumbs.clone()));
                }
                Err(e) => {
                    self.failed.borrow_mut().insert(exam.clone(), e);
                }
            }
        }
        let map = self.opened.borrow();
        map.get(exam).map(f)
    }

    fn with_mut<R>(
        &mut self,
        exam: &ExamId,
        f: impl FnOnce(&mut Opened) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        if self.busy(exam) {
            return Err(BackendError::Denied(format!(
                "Scans are being added to {exam}: nothing can change until the Scan screen says \
                 the batch is complete"
            )));
        }
        // open it first (a command may be the first touch, e.g. from a test)
        self.with(exam, |_| ());
        if let Some(why) = self.failed.borrow().get(exam) {
            return Err(BackendError::Denied(why.clone()));
        }
        let mut map = self.opened.borrow_mut();
        let o = map.get_mut(exam).ok_or_else(|| {
            BackendError::NotFound(format!(
                "Exam {exam} is not open yet: wait for grading to finish, or open its folder \
                 with --project"
            ))
        })?;
        o.writable()?;
        let r = f(o);
        o.changed();
        r
    }

    /// Makes sure the flag stability of `exam`'s current score run is known or being computed
    /// (§12.2, §12.3): a background job on the binary's threads, never on the synchronous
    /// regrade path; without a spawner (headless tests) it runs here, once per score run.
    fn ensure_stability(&self, exam: &ExamId) {
        let Some((run_id, known)) = self.with(exam, Opened::stability_state).flatten() else {
            return;
        };
        if known
            || self
                .stability_jobs
                .borrow()
                .iter()
                .any(|j| &j.exam == exam && j.run_id == run_id)
        {
            return;
        }
        let Some((run_id, data, flags)) = self.with(exam, Opened::stability_input).flatten() else {
            return;
        };
        if let Some(spawn) = &self.spawn
            && flags.iter().any(|f| f.rank() <= 4)
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let work: Box<dyn FnOnce() + Send> = Box::new(move || {
                let report = omr_analytics::stability_job(
                    &data,
                    &flags,
                    &omr_analytics::AnalyticsConfig::default(),
                );
                let _ = tx.send(report);
            });
            if spawn(work).is_ok() {
                self.stability_jobs.borrow_mut().push(StabilityJob {
                    exam: exam.clone(),
                    run_id,
                    rx,
                });
                return;
            }
        }
        // no thread to run it on (or nothing to assess): here, once per score run
        let Some((run_id, data, flags)) = self.with(exam, Opened::stability_input).flatten() else {
            return;
        };
        let report =
            omr_analytics::stability_job(&data, &flags, &omr_analytics::AnalyticsConfig::default());
        if let Some(o) = self.opened.borrow_mut().get_mut(exam) {
            o.set_stability(run_id, report.clone());
            o.judge(report);
        }
    }

    /// Folds finished stability jobs (`wait`: blocks until every running one has reported).
    fn drain_stability(&self, wait: bool) -> bool {
        let jobs = std::mem::take(&mut *self.stability_jobs.borrow_mut());
        let mut changed = false;
        let mut still = Vec::new();
        for j in jobs {
            let got = if wait {
                j.rx.recv().map_err(|_| TryRecvError::Disconnected)
            } else {
                j.rx.try_recv()
            };
            match got {
                Ok(report) => {
                    if let Some(o) = self.opened.borrow_mut().get_mut(&j.exam) {
                        o.set_stability(j.run_id.clone(), report.clone());
                        o.judge(report);
                        changed = true;
                    }
                }
                Err(TryRecvError::Empty) => still.push(j),
                // the job died: its flags read "not assessed" for this run, never retried in
                // a loop
                Err(TryRecvError::Disconnected) => {
                    if let Some(o) = self.opened.borrow().get(&j.exam) {
                        o.set_stability(
                            j.run_id.clone(),
                            omr_analytics::StabilityReport {
                                method: omr_analytics::StabilityMethod::Bootstrap,
                                n: 0,
                                resamples: 0,
                                run_id: j.run_id.clone(),
                                seed: String::new(),
                                flags: Vec::new(),
                                item_ci: Vec::new(),
                            },
                        );
                        changed = true;
                    }
                }
            }
        }
        self.stability_jobs.borrow_mut().extend(still);
        changed
    }

    /// Folds the worker's events; returns true when anything changed.
    fn drain_job(&mut self) -> bool {
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut finished = None;
        loop {
            match job.rx.try_recv() {
                Ok(JobEvent::Progress(p)) => {
                    let now = snapshot_label(&omr_app::pipeline::now_utc());
                    match &p {
                        Progress::Started { pages, workers } => job
                            .activity
                            .insert(0, format!("{now} · {pages} pages on {workers} workers")),
                        Progress::Stage(s) => job.activity.insert(0, format!("{now} · {s}")),
                        Progress::Finished { total_ms } => job
                            .activity
                            .insert(0, format!("{now} · recognised in {total_ms:.0} ms")),
                        Progress::Page { page, .. } => {
                            // the tracker's classification, kept for every page (the tracker
                            // keeps only the latest thumbnails)
                            let state = match &page.worst_code {
                                Some((_, s))
                                    if *s == AppSeverity::Blocker && page.questions == 0 =>
                                {
                                    2
                                }
                                Some((_, s)) if *s >= AppSeverity::Error => 1,
                                _ if page.auto_accepted < page.questions => 1,
                                _ => 0,
                            };
                            job.sheets.insert(page.page_id.clone(), state);
                            // page notices that never reach the queue (§8.3 orientation)
                            if let Some((code, AppSeverity::Info)) = &page.worst_code
                                && code == "REG-002"
                            {
                                job.activity.insert(
                                    0,
                                    format!(
                                        "{now} · page {} · REG-002 read turned upright",
                                        short_page(&page.page_id)
                                    ),
                                );
                            }
                        }
                    }
                    job.tracker.push(&p);
                    changed = true;
                }
                Ok(JobEvent::Note(line)) => {
                    let now = snapshot_label(&omr_app::pipeline::now_utc());
                    job.activity.insert(0, format!("{now} · {line}"));
                    changed = true;
                }
                Ok(JobEvent::Finished(r)) => {
                    finished = Some(r);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if job.outcome.is_none() {
                        finished = Some(Err("The grading worker stopped without a result. Run \
                                             the same `omr grade` (or `omr add`) command to see \
                                             the error"
                            .to_string()));
                    }
                    break;
                }
            }
        }
        job.activity.truncate(40);
        let Some(r) = finished else {
            return changed;
        };
        let exam = job.exam.clone();
        let now = snapshot_label(&omr_app::pipeline::now_utc());
        match &r {
            Ok(dir) => {
                job.activity
                    .insert(0, format!("{now} · saved {}", dir.display()));
            }
            Err(e) => job.activity.insert(0, format!("{now} · failed: {e}")),
        }
        job.outcome = Some(r.clone());
        // a new project opens on success; an added batch reopens its project either way (a
        // refused batch leaves the exam as it was)
        let dir = match (&job.kind, r) {
            (JobKind::Append { project, .. }, _) => Some(project.clone()),
            (JobKind::Grade { .. }, Ok(dir)) => Some(dir),
            (JobKind::Grade { .. }, Err(_)) => None,
        };
        if let Some(dir) = dir {
            match open_project(&dir, &self.operator) {
                Ok((p, ro)) => {
                    let summary = p.summary_vm();
                    // the job's exam ID stays, so the open screens stay on it
                    self.entries.retain(|e| e.dir != dir);
                    self.entries.insert(
                        0,
                        Entry {
                            id: exam.clone(),
                            dir: dir.clone(),
                            summary,
                            snapshot_utc: p.session.snapshot_utc.clone(),
                            pages: p.session.timings.pages,
                            page_stage_ms: p.session.timings.page_stage_ms,
                        },
                    );
                    self.failed.borrow_mut().remove(&exam);
                    self.opened
                        .borrow_mut()
                        .insert(exam.clone(), Opened::new(p, dir, ro, self.thumbs.clone()));
                }
                Err(e) => {
                    self.failed.borrow_mut().insert(exam.clone(), e);
                }
            }
            self.reloaded.push(exam);
        }
        true
    }

    /// Starts adding `inputs` to `exam` (see [`Backend::add_scans`]).
    fn start_append(&mut self, exam: &ExamId, inputs: &[PathBuf]) -> Result<String, BackendError> {
        if inputs.is_empty() {
            return Err(BackendError::Invalid(
                "Type the folder or the scan files to add (several: separate them with ;)".into(),
            ));
        }
        if let Some(j) = self.job.as_ref().filter(|j| j.running()) {
            return Err(BackendError::Denied(format!(
                "{} is still being graded: add the scans when the Scan screen says complete",
                j.exam
            )));
        }
        if self.spawn.is_none() {
            return Err(BackendError::Denied(
                "This window cannot start background work; add the scans with `omr add \
                 --project <exam folder> <scans…>` and reopen the exam"
                    .into(),
            ));
        }
        // the project must be open for writing; it is saved and closed for the worker
        self.with_mut(exam, |_| Ok(()))?;
        let Some(o) = self.opened.borrow_mut().remove(exam) else {
            return Err(BackendError::NotFound(format!("Exam {exam} is not open")));
        };
        let project = o.dir.clone();
        // "batch-2" → the new batch is 3
        let batch =
            o.p.store()
                .and_then(|st| st.batch_id().strip_prefix("batch-")?.parse::<u32>().ok())
                .unwrap_or(1)
                + 1;
        let threads = o.p.session.timings.threads;
        if let Err(e) = o.p.close() {
            // nothing was handed over: reopen it as it was
            self.with(exam, |_| ());
            return Err(app_err(e));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let req = AppendRequest {
            project: project.clone(),
            inputs: inputs.to_vec(),
            options: GradeOptions {
                threads,
                project_dir: None,
                snapshot_utc: None,
                operator: self.operator.clone(),
                max_pages: None,
                cancel: Default::default(),
            },
        };
        let started =
            (self.spawn.as_ref().expect("checked above"))(Box::new(move || run_append(req, tx)));
        if let Err(e) = started {
            self.with(exam, |_| ());
            return Err(BackendError::Denied(format!(
                "Cannot start the grading worker ({e}); nothing was added. Close other \
                 programs and try again, or use `omr add`"
            )));
        }
        let names: Vec<String> = inputs
            .iter()
            .map(|p| {
                p.file_name()
                    .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
            })
            .collect();
        let now = snapshot_label(&omr_app::pipeline::now_utc());
        self.job = Some(Job {
            exam: exam.clone(),
            kind: JobKind::Append { project, batch },
            rx,
            tracker: ProgressTracker::default(),
            activity: vec![format!(
                "{now} · batch-{batch} · adding {}; every page is graded again",
                names.join(", ")
            )],
            outcome: None,
            sheets: BTreeMap::new(),
        });
        Ok(format!(
            "Adding scans as batch {batch}: the exam is graded again with them in the background; \
             review decisions carry over"
        ))
    }
}

/// The `project.omrproj` package of an exam folder (or the package itself).
pub fn project_dir(dir: &Path) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "omrproj") {
        dir.to_path_buf()
    } else {
        dir.join("project.omrproj")
    }
}

/// Opens read-write, else, when another program holds the writer, read-only with the reason;
/// errors carry remediation text. A project that fails its checks (SYS-010, SYS-011 audit
/// chain or snapshot, SYS-015 location) is not opened at all: its error says what happened and
/// what to do, never "close the other program".
fn open_project(dir: &Path, operator: &str) -> Result<(ExamProject, Option<String>), String> {
    match ExamProject::open(dir, operator) {
        Ok(p) => Ok((p, None)),
        Err(e @ AppError::NotFound(_)) => Err(format!(
            "{e}. Point --project at an exam folder that `omr grade --out` or `omr-desktop \
             --grade` wrote"
        )),
        Err(AppError::Store(m)) if m.contains("locked by another writer") => {
            match ExamProject::open_read_only(dir) {
                Ok(p) => Ok((
                    p,
                    Some("another program has it open for writing".to_string()),
                )),
                Err(e2) => Err(format!("{} cannot be opened: {e2}", dir.display())),
            }
        }
        Err(e) => Err(format!("{} cannot be opened: {e}", dir.display())),
    }
}

// ------------------------------------------------------------------------------------ mapping

fn sev(s: AppSeverity) -> b::Severity {
    match s {
        AppSeverity::Info => b::Severity::Info,
        AppSeverity::Warn => b::Severity::Warn,
        AppSeverity::Error => b::Severity::Error,
        AppSeverity::Blocker => b::Severity::Blocker,
    }
}

fn lane_to_app(l: b::Lane) -> AppLane {
    match l {
        b::Lane::Marks => AppLane::Marks,
        b::Lane::Identity => AppLane::Identity,
        b::Lane::Capture => AppLane::Capture,
        b::Lane::Scoring => AppLane::Scoring,
    }
}

fn lane_of_app(l: AppLane) -> b::Lane {
    match l {
        AppLane::Marks => b::Lane::Marks,
        AppLane::Identity => b::Lane::Identity,
        AppLane::Capture => b::Lane::Capture,
        AppLane::Scoring => b::Lane::Scoring,
    }
}

fn kind(k: ItemKind) -> b::ReviewKind {
    match k {
        ItemKind::Mark => b::ReviewKind::Mark,
        ItemKind::Identity => b::ReviewKind::Identity,
        ItemKind::Form => b::ReviewKind::Form,
        ItemKind::Notice => b::ReviewKind::Notice,
    }
}

fn mark_state(s: OverlayState) -> b::MarkState {
    match s {
        OverlayState::Filled => b::MarkState::Filled,
        OverlayState::Ambiguous => b::MarkState::Ambiguous,
        OverlayState::Unreadable => b::MarkState::Unreadable,
        OverlayState::Blank => b::MarkState::Blank,
    }
}

fn counts(c: vm::SeverityCounts) -> b::SeverityCounts {
    b::SeverityCounts {
        info: c.info,
        warn: c.warn,
        error: c.error,
        blocker: c.blocker,
    }
}

fn status(s: vm::ExamStatus) -> b::ExamStatus {
    match s {
        vm::ExamStatus::Setup => b::ExamStatus::Setup,
        vm::ExamStatus::Scanning => b::ExamStatus::Scanning,
        vm::ExamStatus::InReview => b::ExamStatus::InReview,
        vm::ExamStatus::Ready => b::ExamStatus::Ready,
        vm::ExamStatus::Released => b::ExamStatus::Released,
    }
}

fn crop(c: &vm::CropImage) -> b::CropImage {
    b::CropImage {
        width: c.width,
        height: c.height,
        rgba: c.rgba.clone(),
    }
}

/// What to do about a page whose alignment with the blank form could not be verified (REG-003
/// ERROR: a bubble lattice can also match one row or column away, so nothing on the page was
/// read): the core's remedy (R10).
fn alignment_remedy() -> &'static str {
    omr_app::review::remedy("REG-003").unwrap_or("rescan the sheet flat and add the new scan")
}

/// The page-level REG-003 ERROR of a page read on an alignment that could not be verified, if
/// any (a region-level REG-003 only makes its bubbles unreadable).
fn alignment_refusal(p: &omr_app::model::PageState) -> Option<&omr_model::Exception> {
    refusal_in(&p.read.as_ref()?.exceptions)
}

fn refusal_in(ex: &[omr_model::Exception]) -> Option<&omr_model::Exception> {
    ex.iter().find(|e| {
        e.code == "REG-003" && e.severity >= AppSeverity::Error && e.scope == omr_model::Scope::Page
    })
}

/// The activity line of a page read on no verified alignment, with what to do (R10).
fn alignment_notice(serial: &str) -> String {
    format!(
        "REG-003 {serial} · not read: alignment with the blank form not verified · {}",
        alignment_remedy()
    )
}

/// What a confirmation of the exam's own key sheets did, in the desktop's words.
fn confirmed_message(r: &omr_app::keysheet::KeyConfirmReport) -> String {
    let mut m = if !r.held {
        format!(
            "Key confirmed: key version {} published from the key sheets with your name; the \
             exam is scored",
            r.key_version.unwrap_or(1)
        )
    } else if r.waiting.is_empty() {
        format!(
            "Version {} confirmed and recorded with your name. The exam stays held: a key sheet \
             could not be read (see Key sheets); rescan it, or replace the key from a key file",
            r.confirmed.join(" and ")
        )
    } else {
        format!(
            "Version {} confirmed and recorded with your name; version {} {} for your \
             confirmation: nothing is scored until then",
            r.confirmed.join(" and "),
            r.waiting.join(" and "),
            if r.waiting.len() == 1 {
                "waits"
            } else {
                "wait"
            }
        )
    };
    if !r.warnings.is_empty() {
        m.push_str(&format!(". Check: {}", r.warnings.join("; ")));
    }
    m
}

/// A key-sheet row crop with its mark overlays drawn on the paper (one image per row).
fn marked_crop(c: &omr_app::crops::Crop) -> b::CropImage {
    let plain = b::CropImage {
        width: c.width,
        height: c.height,
        rgba: c.rgba.clone(),
    };
    let marks: Vec<crate::crops::MarkOverlay> = c
        .bubbles
        .iter()
        .map(|o| crate::crops::MarkOverlay {
            cx: o.cx,
            cy: o.cy,
            r: o.r,
            state: mark_state(o.state),
        })
        .collect();
    crate::crops::with_marks(&plain, &marks)
}

/// The stability of one flag from the stability job's report (§12.2): rules of ranks 1–4 are
/// assessed; "computing…" until the job has reported.
fn flag_stability(
    f: &omr_analytics::ItemFlag,
    report: Option<&omr_analytics::StabilityReport>,
) -> b::FlagStability {
    if f.rank() > 4 {
        return b::FlagStability::NotAssessed;
    }
    let Some(r) = report else {
        return b::FlagStability::Computing;
    };
    match r
        .flags
        .iter()
        .find(|s| s.item == f.item && s.rule == f.rule && s.option == f.option)
    {
        None => b::FlagStability::NotAssessed,
        Some(s) if s.low_confidence => b::FlagStability::Low {
            share: s.share,
            note: s.describe(r.resamples),
        },
        Some(s) => b::FlagStability::High {
            share: s.share,
            note: s.describe(r.resamples),
        },
    }
}

fn key_row_status(s: omr_app::keysheet::RowStatus) -> b::KeyRowStatus {
    use omr_app::keysheet::RowStatus as R;
    match s {
        R::Read => b::KeyRowStatus::Read,
        R::Decided => b::KeyRowStatus::Decided,
        R::MultiMark => b::KeyRowStatus::MultiMark,
        R::Blank => b::KeyRowStatus::Blank,
        R::NotPrinted => b::KeyRowStatus::NotPrinted,
        R::Uncertain => b::KeyRowStatus::Uncertain,
        R::Disagree => b::KeyRowStatus::Disagree,
    }
}

/// Two key entries say the same (`A|C` = `C,A`; a marked inferred entry compares by its
/// letters).
fn same_entry(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim_end_matches(" (inferred)");
        let mut v: Vec<char> = s
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '*')
            .collect();
        v.sort_unstable();
        v
    };
    norm(a) == norm(b)
}

fn title_of(code: &str) -> String {
    omr_model::exceptions::lookup(code).map_or_else(String::new, |d| d.title.to_string())
}

/// An exact rational of the outcome engine (`Ratio<i128>`) as `f64`, for display only.
macro_rules! exact {
    ($x:expr) => {
        *$x.numer() as f64 / *$x.denom() as f64
    };
}

/// The digits of a student ID (roster cells may carry spaces or a leading apostrophe).
fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// RFC 3339 UTC "2026-10-12T09:00:00Z" → "2026-10-12 09:00 UTC".
fn snapshot_label(s: &str) -> String {
    match (s.get(..10), s.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t} UTC"),
        _ => s.to_string(),
    }
}

/// Days since 1970-01-01 of an RFC 3339 date (civil calendar, proleptic Gregorian).
fn epoch_day(s: &str) -> Option<i64> {
    let y: i64 = s.get(..4)?.parse().ok()?;
    let m: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The printed version letter of version bubble `i` (the IAU form prints A–D in order).
fn printed(i: u8) -> String {
    char::from(b'A' + i.min(25)).to_string()
}

/// Shell decision → core decision for an item of kind `k` (§13.3 keys). A version bubble maps
/// to the exam's form with that printed letter (the definition may list forms in any order).
fn to_app(d: b::Decision, k: ItemKind, forms: &[String]) -> Result<AppDecision, BackendError> {
    Ok(match (d, k) {
        (b::Decision::Option(o), ItemKind::Form) => {
            let f = forms.iter().position(|f| *f == printed(o)).ok_or_else(|| {
                BackendError::Invalid(format!(
                    "Version {} is printed on the sheet but this exam has only {}: choose one \
                     of them",
                    printed(o),
                    forms.join(", ")
                ))
            })?;
            AppDecision::Form(f as u8)
        }
        (b::Decision::Option(o), _) => AppDecision::Option(o),
        (b::Decision::Blank, _) => AppDecision::Blank,
        // `M` without options: the core takes the marked options (its proposal, or every
        // bubble in the review band)
        (b::Decision::Multi, _) => AppDecision::Multi(Vec::new()),
        (b::Decision::Unreadable, _) => AppDecision::Unreadable,
        // a candidate is the n-th of the item's listed candidates, as the core lists them
        (b::Decision::Candidate(n), _) => AppDecision::Option(n),
        (b::Decision::NotOnRoster, _) => AppDecision::NotOnRoster,
        (b::Decision::Duplicate, _) => {
            return Err(BackendError::Invalid(
                "Duplicate (D) is not recorded by omr-app yet: choose the student this booklet \
                 belongs to (1–3) or N (not on roster); two booklets can never both keep one \
                 student"
                    .into(),
            ));
        }
        (b::Decision::Acknowledge, _) => AppDecision::Acknowledge,
    })
}

/// Core decision → shell decision; `candidates` are the booklet's listed student keys, `forms`
/// the exam's form letters (a form shows as the version bubble printed with its letter).
fn of_app(d: &AppDecision, k: ItemKind, candidates: &[String], forms: &[String]) -> b::Decision {
    match d {
        AppDecision::Option(o) if k == ItemKind::Identity => b::Decision::Candidate(*o),
        AppDecision::Form(f) => b::Decision::Option(
            forms
                .get(usize::from(*f))
                .and_then(|l| l.bytes().next())
                .map_or(*f, |c| c.saturating_sub(b'A')),
        ),
        AppDecision::Option(o) => b::Decision::Option(*o),
        AppDecision::Blank => b::Decision::Blank,
        AppDecision::Multi(_) => b::Decision::Multi,
        AppDecision::Unreadable => b::Decision::Unreadable,
        AppDecision::Student(s) => {
            let s = digits(s);
            b::Decision::Candidate(
                candidates
                    .iter()
                    .position(|c| digits(c) == s)
                    .map_or(u8::MAX, |i| i.min(254) as u8),
            )
        }
        AppDecision::NotOnRoster => b::Decision::NotOnRoster,
        // keeping or voiding a probable mis-key (SCO-001) reads as the item's acknowledgement
        AppDecision::Acknowledge | AppDecision::Void => b::Decision::Acknowledge,
    }
}

fn action_of_app(
    a: &Action,
    k: ItemKind,
    candidates: &[String],
    forms: &[String],
) -> b::ReviewAction {
    match a {
        Action::Resolve {
            decision,
            accepted_suggestion,
            note,
        } => b::ReviewAction::Resolve {
            decision: of_app(decision, k, candidates, forms),
            accepted_suggestion: *accepted_suggestion,
            note: note.clone(),
        },
        Action::ApplyToGroup { decision } => b::ReviewAction::ApplyToGroup {
            decision: of_app(decision, k, candidates, forms),
        },
        Action::Defer => b::ReviewAction::Defer,
        Action::Escalate => b::ReviewAction::Escalate,
        Action::AddToRescan => b::ReviewAction::AddToRescan,
    }
}

fn label_set(labels: &[String]) -> b::LabelSet {
    match labels.first().and_then(|l| l.chars().next()) {
        Some(c) if c.is_ascii_digit() => b::LabelSet::Numeric,
        Some(c) if c.is_ascii() => b::LabelSet::Latin,
        Some(_) => b::LabelSet::Arabic,
        None => b::LabelSet::Latin,
    }
}

fn domain(d: DomainLetter) -> b::NqfDomain {
    match d {
        DomainLetter::K => b::NqfDomain::Knowledge,
        DomainLetter::S => b::NqfDomain::Skills,
        DomainLetter::V => b::NqfDomain::Values,
    }
}

/// "Q3–5, Q12 · 4 items · Σw 3.5" for one CLO of the definition's mapping.
fn mapping_note(def: &ExamDefinition, clo: &str) -> String {
    let Some(o) = &def.outcomes else {
        return String::new();
    };
    let mut qs: Vec<(u16, f64)> = o
        .mapping
        .iter()
        .filter(|m| m.clo == clo)
        .map(|m| (m.question, parse_weight(&m.weight)))
        .collect();
    qs.sort_by_key(|q| q.0);
    if qs.is_empty() {
        return "no items mapped".into();
    }
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for &(q, _) in &qs {
        match runs.last_mut() {
            Some(r) if r.1 + 1 == q => r.1 = q,
            _ => runs.push((q, q)),
        }
    }
    let text: Vec<String> = runs
        .iter()
        .map(|&(a, z)| {
            if a == z {
                format!("Q{a}")
            } else {
                format!("Q{a}–{z}")
            }
        })
        .collect();
    let sum: f64 = qs.iter().map(|q| q.1).sum();
    format!(
        "{} · {} item{} · Σw {}",
        text.join(", "),
        qs.len(),
        if qs.len() == 1 { "" } else { "s" },
        trim(sum)
    )
}

/// The lowest percentage of a passing band of the scale in force.
fn pass_mark(grades: &[vm::GradeBand]) -> f64 {
    grades
        .iter()
        .filter(|g| !g.fail)
        .map(|g| g.lo)
        .fold(f64::INFINITY, f64::min)
        .min(100.0)
}

/// "1 forms inferred" → "1 form inferred": the core's readiness counts read in the plural.
fn singular(text: &str) -> String {
    let Some(rest) = text.strip_prefix("1 ") else {
        return text.to_string();
    };
    let mut words: Vec<&str> = rest.split(' ').collect();
    for w in words.iter_mut().take(3) {
        if let Some(one) = match *w {
            "forms" => Some("form"),
            "responses" => Some("response"),
            "totals" => Some("total"),
            "pages" => Some("page"),
            "items" => Some("item"),
            "booklets" => Some("booklet"),
            _ => None,
        } {
            *w = one;
            break;
        }
    }
    format!("1 {}", words.join(" "))
}

/// "1", "0.5", "1/3" → f64 (mapping weights are exact strings in the definition).
fn parse_weight(w: &str) -> f64 {
    match w.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (
                n.trim().parse().unwrap_or(0.0),
                d.trim().parse().unwrap_or(1.0),
            );
            if d == 0.0 { 0.0 } else { n / d }
        }
        None => w.trim().parse().unwrap_or(0.0),
    }
}

fn trim(x: f64) -> String {
    let t = format!("{x:.2}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ------------------------------------------------------------------------------------ views

/// The worst open item (code, severity) per page or booklet index.
type Worst = BTreeMap<usize, (String, AppSeverity)>;

impl Opened {
    fn summary(&self, entry: Option<&Entry>) -> b::ExamSummary {
        let v = self.p.summary_vm();
        let mut s = summary_of(&v, &self.p.session.snapshot_utc);
        if let Some(e) = entry {
            s.id = e.id.clone();
        }
        s
    }

    fn setup(&self, exam: &ExamId) -> b::ExamSetup {
        let v = self.p.setup_vm();
        let def = &self.p.session.definition;
        let clos: Vec<b::CloDef> = def
            .outcomes
            .as_ref()
            .map(|o| {
                o.clos
                    .iter()
                    .map(|c| b::CloDef {
                        code: c.code.clone(),
                        title: c.title.clone(),
                        title_ar: c.title_ar.clone(),
                        domain: domain(c.domain),
                        plo: c.plo.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // an exam without CLOs still lists its questions, so its key is shown and editable
        let mut mapping: Vec<b::MappingRow> = v
            .mapping
            .iter()
            .map(|r| b::MappingRow {
                question: r.question,
                points: r.points,
                cells: r
                    .cells
                    .iter()
                    .map(|(w, n)| b::MappingCell {
                        weight: *w,
                        note: n.clone(),
                    })
                    .collect(),
                // imported rows are proposed until approved (§11.2); the coordinator's own
                // rows are the mapping in force (attainment uses them), never "approved"
                status: {
                    let rows: Vec<&omr_app::CloMapping> = def
                        .outcomes
                        .as_ref()
                        .map(|o| {
                            o.mapping
                                .iter()
                                .filter(|m| u32::from(m.question) == r.question)
                                .collect()
                        })
                        .unwrap_or_default();
                    if rows
                        .iter()
                        .any(|m| !m.imported.is_empty() && m.approved_by.is_empty())
                    {
                        b::MappingStatus::Proposed
                    } else if rows.iter().any(|m| !m.approved_by.is_empty()) {
                        b::MappingStatus::Approved
                    } else if r.cells.iter().any(|c| c.0.is_some()) {
                        b::MappingStatus::InUse
                    } else {
                        b::MappingStatus::Draft
                    }
                },
            })
            .collect();
        if mapping.is_empty() {
            mapping = (1..=u32::from(def.questions))
                .map(|q| b::MappingRow {
                    question: q,
                    points: def
                        .points
                        .get(q as usize - 1)
                        .map_or(1.0, |p| parse_weight(p)),
                    cells: Vec::new(),
                    status: b::MappingStatus::Draft,
                })
                .collect();
        }
        b::ExamSetup {
            exam: exam.clone(),
            roster_students: def.roster.as_ref().map_or(0, |r| r.len() as u32),
            mappings_proposed: def.outcomes.as_ref().map_or(0, |o| {
                o.mapping
                    .iter()
                    .filter(|m| !m.imported.is_empty() && m.approved_by.is_empty())
                    .count() as u32
            }),
            template: b::TemplateInfo {
                id: v.template.id.clone(),
                kind: if v.template.legacy {
                    b::TemplateKind::Legacy
                } else {
                    b::TemplateKind::Generated
                },
                name: v.template.name.clone(),
                questions: v.template.questions,
                capacity: v.template.capacity,
                label_set: label_set(&v.template.option_labels),
                option_labels: v.template.option_labels.clone(),
                versions: v.template.versions.clone(),
                cancel_mode: match def.cancel_mode {
                    omr_model::CancelMode::EraseOnly => b::CancelMode::EraseOnly,
                    omr_model::CancelMode::CrossCancels => b::CancelMode::CrossCancels,
                    omr_model::CancelMode::FillCancelsCheck => b::CancelMode::FillCancelsCheck,
                },
                dpi: v.template.dpi,
                page_identity: v.template.page_identity.clone(),
                digest: v.template.digest.clone(),
            },
            keys: v
                .keys
                .iter()
                .map(|k| b::VersionKey {
                    version: k.version.clone(),
                    answers: k.answers.clone(),
                })
                .collect(),
            key_status: v.key_status.clone(),
            key_published: v.key_published,
            clos,
            mapping_mode: if v.mapping_split {
                b::MappingMode::Split
            } else {
                b::MappingMode::Full
            },
            mapping,
            lints: v
                .lints
                .iter()
                .map(|(code, s, m)| b::SetupLint {
                    code: code.clone(),
                    severity: sev(*s),
                    message: m.clone(),
                })
                .collect(),
        }
    }

    /// The key read from key sheets (§10.1): files read for this exam and not confirmed yet,
    /// else the exam's own key sheets. Cached with its crops until the next change.
    fn key_summary(&self) -> Option<b::KeySheetSummary> {
        if let Some(hit) = self.keysheets.borrow().as_ref() {
            return hit.clone();
        }
        let made = self.make_key_summary();
        *self.keysheets.borrow_mut() = Some(made.clone());
        made
    }

    /// The exam's key entry for (form, 0-based question) now, as `B`, `A|C`, `*` or empty;
    /// `INFERRED` keys are marked so.
    fn current_entry(&self, form: usize, q: usize) -> Option<String> {
        let k = self.p.session.key.as_ref()?;
        let cells = k.forms.get(if k.shared_items { 0 } else { form })?;
        let t = cells.get(q)?.text();
        Some(if k.inferred && !t.is_empty() {
            format!("{t} (inferred)")
        } else {
            t
        })
    }

    fn make_key_summary(&self) -> Option<b::KeySheetSummary> {
        let (p, preview) = match &self.preview {
            Some(pv) => (&pv.p, Some(pv)),
            None => (&self.p, None),
        };
        let sum = p.key_sheet_summary(true)?;
        let forms = &p.session.definition.forms;
        let scan = |k: &omr_app::keysheet::KeySheetInfo| b::KeySheetScan {
            label: k.label.clone(),
            origin: if preview.is_some() {
                "read from the file".into()
            } else if k.declared {
                "named as a key sheet".into()
            } else {
                "found in the stack".into()
            },
            id_read: k.id_read.clone(),
            version: k.version_text.clone(),
            registration: k.registration.clone(),
            digest: format!(
                "sha384 {}…{}",
                k.sha384.get(..6).unwrap_or(""),
                k.sha384
                    .get(k.sha384.len().saturating_sub(4)..)
                    .unwrap_or("")
            ),
            refusal: k.refusal.clone(),
        };
        let versions = sum
            .versions
            .iter()
            .map(|v| {
                let form = forms.iter().position(|f| f == &v.version).unwrap_or(0);
                b::KeySheetVersion {
                    version: v.version.clone(),
                    form,
                    sheets: v.sheets.iter().map(scan).collect(),
                    rows: v
                        .rows
                        .iter()
                        .map(|r| {
                            let q = r.question as usize - 1;
                            // the exam's own key when it differs: from the core for its key
                            // sheets, from the exam's current key for files read in preview
                            let key_file = match preview {
                                None => r.key_file.clone(),
                                Some(_) => self
                                    .current_entry(form, q)
                                    .filter(|now| !same_entry(now, &r.answer)),
                            };
                            let marked: Vec<u8> = r
                                .crop
                                .as_ref()
                                .map(|c| {
                                    c.bubbles
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, b)| {
                                            matches!(
                                                b.state,
                                                OverlayState::Filled | OverlayState::Ambiguous
                                            )
                                        })
                                        .map(|(i, _)| i as u8)
                                        .collect()
                                })
                                .unwrap_or_default();
                            b::KeySheetRow {
                                question: r.question,
                                read: r.read.clone(),
                                answer: r.answer.clone(),
                                status: key_row_status(r.status),
                                detail: r.detail.clone(),
                                key_file,
                                fills: r.fills.clone(),
                                marked,
                                crop: r.crop.as_ref().map(marked_crop),
                            }
                        })
                        .collect(),
                    open: v.open as u32,
                    confirmed: v.confirmed.as_ref().map(|(by, at)| {
                        format!(
                            "confirmed by {} · {}",
                            by.trim_start_matches("principal:"),
                            snapshot_label(at)
                        )
                    }),
                }
            })
            .collect();
        let source = match preview {
            Some(pv) => format!(
                "Read from {} · not confirmed: the exam's key is unchanged until you confirm",
                match pv.files.as_slice() {
                    [one] => one.clone(),
                    many => format!("{} key-sheet files", many.len()),
                }
            ),
            None if sum.superseded => {
                "Key sheets graded with the exam · a key file replaced their key".into()
            }
            None if sum.pending => {
                "Key sheets graded with the exam · not confirmed: nothing is scored until you \
                 confirm"
                    .into()
            }
            None => "Key sheets graded with the exam · confirmed".into(),
        };
        let consequence = match preview {
            Some(_) => "Confirming replaces the exam's key with this one (a new key version, \
                        recorded with your name and the key-sheet files) and rescores the exam."
                .to_string(),
            None => "Confirming records your confirmation with your name and the key-sheet \
                     scans; when the last version is confirmed the key is published and the \
                     exam is scored."
                .to_string(),
        };
        Some(b::KeySheetSummary {
            source,
            preview: preview.is_some(),
            pending: sum.pending,
            superseded: sum.superseded,
            versions,
            unusable: sum.unusable.iter().map(scan).collect(),
            other_versions: sum.other_versions.clone(),
            consequence,
            notes: sum.notes.clone(),
        })
    }

    /// The analysis input and the synchronous flags of the current score run, for the
    /// stability job (§12.2).
    fn stability_input(
        &self,
    ) -> Option<(
        String,
        omr_analytics::AnalysisData,
        Vec<omr_analytics::ItemFlag>,
    )> {
        let s = self.p.scoring.as_ref()?;
        let run = s.run.as_ref().ok()?;
        let data = omr_analytics::AnalysisData::from_run(run, &s.prep.matrix);
        let flags = omr_analytics::analyze(&data, &omr_analytics::AnalyticsConfig::default()).flags;
        Some((run.run_id.clone(), data, flags))
    }

    /// The current score run's ID, and whether its flag stability is known.
    fn stability_state(&self) -> Option<(String, bool)> {
        let run = self.p.scoring.as_ref()?.run.as_ref().ok()?;
        let known = self
            .stability
            .borrow()
            .as_ref()
            .is_some_and(|(id, _)| id == &run.run_id);
        Some((run.run_id.clone(), known))
    }

    /// Stores a stability report for `run_id`; the results are rebuilt with it.
    fn set_stability(&self, run_id: String, report: omr_analytics::StabilityReport) {
        *self.stability.borrow_mut() = Some((run_id, report));
        *self.results.borrow_mut() = None;
    }

    /// Hands a stability report to the core, which judges the probable mis-keys of the score
    /// run for the release gate (§12.2: a confident one becomes an SCO-001 review item that
    /// blocks release until decided). A read-only exam keeps "computing…".
    fn judge(&mut self, report: omr_analytics::StabilityReport) {
        if self.read_only.is_none() && self.p.set_flag_stability(report).is_ok() {
            self.changed();
        }
    }

    /// The flag stability of the current run, when the job has reported.
    fn current_stability(&self) -> Option<omr_analytics::StabilityReport> {
        let run = self.p.scoring.as_ref()?.run.as_ref().ok()?;
        self.stability
            .borrow()
            .as_ref()
            .filter(|(id, _)| id == &run.run_id)
            .map(|(_, r)| r.clone())
    }

    /// A page thumbnail (decoded from the stored original once, then cached).
    fn thumbnail(&self, page: usize) -> Option<b::CropImage> {
        let key = (
            self.dir.clone(),
            self.p.session.pages.get(page)?.page_id.clone(),
        );
        if let Some(hit) = self.thumbs.borrow().get(&key) {
            return hit.clone();
        }
        let made = self.p.page_image(page).ok().map(|img| {
            let c = omr_app::crops::thumbnail(&img, THUMB_WIDTH);
            b::CropImage {
                width: c.width,
                height: c.height,
                rgba: c.rgba,
            }
        });
        self.thumbs.borrow_mut().insert(key, made.clone());
        made
    }

    /// Worst open blocking item per page index and per booklet.
    fn open_codes(&self) -> (Worst, Worst) {
        let mut by_page = Worst::new();
        let mut by_booklet = Worst::new();
        for it in self.p.session.review.items.iter().filter(|i| i.is_open()) {
            let worse = |cur: Option<&(String, AppSeverity)>| {
                cur.is_none_or(|(c, s)| it.severity > *s || (it.severity == *s && it.code < *c))
            };
            if let Some(bk) = it.booklet {
                if worse(by_booklet.get(&bk)) {
                    by_booklet.insert(bk, (it.code.clone(), it.severity));
                }
            } else if let Some(pg) = it.page
                && worse(by_page.get(&pg))
            {
                by_page.insert(pg, (it.code.clone(), it.severity));
            }
        }
        (by_page, by_booklet)
    }

    fn thumb_state(&self, page: usize, by_page: &Worst, by_booklet: &Worst) -> b::ThumbState {
        let pg = &self.p.session.pages[page];
        // the instructor's key sheet is never a student sheet (§10.1)
        if pg.status == PageStatus::KeySheet {
            let ks = &self.p.session.key_sheets;
            let sheet = ks.sheets.iter().find(|k| k.pages.contains(&page));
            return b::ThumbState::KeySheet {
                version: sheet.map_or_else(String::new, |k| k.form_text.clone()),
                confirmed: sheet
                    .and_then(|k| k.form)
                    .is_some_and(|f| ks.is_confirmed(f))
                    || ks.superseded,
                refused: sheet.is_none_or(|k| !k.usable()),
            };
        }
        if let Some(e) = &pg.refusal {
            return b::ThumbState::Rejected {
                code: e.code.clone(),
                severity: sev(e.severity),
            };
        }
        let worst = pg
            .booklet
            .and_then(|bk| by_booklet.get(&bk))
            .or_else(|| by_page.get(&page));
        match worst {
            Some((c, s)) => b::ThumbState::Review {
                code: c.clone(),
                severity: sev(*s),
            },
            None => b::ThumbState::Auto,
        }
    }

    fn serial_of_page(&self, page: usize) -> String {
        let s = &self.p.session;
        s.pages[page]
            .booklet
            .map(|bk| format!("{:04}", s.booklets[bk].serial))
            .unwrap_or_else(|| short_page(&s.pages[page].page_id))
    }

    /// The finished batch, from the session (no live tracker).
    fn batch(&self, exam: &ExamId) -> b::BatchProgress {
        let s = &self.p.session;
        let t = &s.timings;
        let questions: usize = s.booklets.iter().map(|b| b.questions.len()).sum();
        let auto = s
            .booklets
            .iter()
            .flat_map(|b| &b.questions)
            .filter(|q| q.status == ResponseStatus::Auto)
            .count();
        let auto_share = if questions == 0 {
            0.0
        } else {
            auto as f64 / questions as f64
        };
        let pps = if t.page_stage_ms > 0.0 {
            t.pages as f64 / (t.page_stage_ms / 1e3)
        } else {
            0.0
        };
        let refused = s
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::Refused)
            .count() as u32;
        let unattributed = s
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::Unattributed)
            .count() as u32;
        let duplicates = s
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::Duplicate)
            .count() as u32;
        let blank = s
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::Blank)
            .count() as u32;
        // exceptions: open review items by code, plus the batch-scope findings
        let mut codes: BTreeMap<(String, AppSeverity), u32> = BTreeMap::new();
        for it in s.review.items.iter().filter(|i| i.is_open()) {
            *codes.entry((it.code.clone(), it.severity)).or_default() += 1;
        }
        for e in &s.batch_exceptions {
            *codes.entry((e.code.clone(), e.severity)).or_default() += 1;
        }
        // the key-sheet hold (SCO-003 BLOCKER) is lifted in Setup, never in the queue, but it
        // holds the whole batch: listed with the codes
        for e in s
            .exam_exceptions
            .iter()
            .filter(|e| e.sub_reason.as_deref() == Some(omr_app::keysheet::HOLD))
        {
            *codes.entry((e.code.clone(), e.severity)).or_default() += 1;
        }
        let mut exceptions: Vec<b::CodeCount> = codes
            .into_iter()
            .map(|((code, sv), count)| b::CodeCount {
                title: title_of(&code),
                code,
                severity: sev(sv),
                count,
            })
            .collect();
        exceptions.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.count.cmp(&a.count))
                .then(a.code.cmp(&b.code))
        });
        let (by_page, by_booklet) = self.open_codes();
        // every sheet once, by its worst open item (the pills over the thumbnails)
        let mut sheets = b::SheetCounts::default();
        for p in 0..s.pages.len() {
            if matches!(s.pages[p].status, PageStatus::Duplicate | PageStatus::Blank) {
                continue;
            }
            match self.thumb_state(p, &by_page, &by_booklet) {
                b::ThumbState::Auto => sheets.auto += 1,
                b::ThumbState::Rejected { .. } => sheets.rejected += 1,
                b::ThumbState::KeySheet { .. } => sheets.key_sheets += 1,
                _ => sheets.review += 1,
            }
        }
        // the instructor's key sheets first (never students), then the highest serials
        // ("latest"), then pages outside any booklet
        let mut pages: Vec<usize> = (0..s.pages.len())
            .filter(|&p| !matches!(s.pages[p].status, PageStatus::Duplicate | PageStatus::Blank))
            .collect();
        pages.sort_by_key(|&p| {
            (
                s.pages[p].status != PageStatus::KeySheet,
                std::cmp::Reverse((s.pages[p].booklet.map(|bk| s.booklets[bk].serial), p)),
            )
        });
        let thumbnails = pages
            .into_iter()
            .take(THUMBS)
            .map(|p| b::Thumbnail {
                serial: self.serial_of_page(p),
                state: self.thumb_state(p, &by_page, &by_booklet),
                image: self.thumbnail(p),
            })
            .collect();
        let warn = |n: u32| {
            if n > 0 {
                b::Tone::Warning
            } else {
                b::Tone::Neutral
            }
        };
        let mut reconciliation = vec![
            b::ReconLine {
                label: "Files imported".into(),
                value: s.sources.len().to_string(),
                tone: b::Tone::Neutral,
            },
            b::ReconLine {
                label: "Pages read".into(),
                value: format!(
                    "{} of {}",
                    s.pages.len() as u32 - refused - duplicates - blank,
                    s.pages.len()
                ),
                tone: b::Tone::Neutral,
            },
            b::ReconLine {
                label: "Booklets".into(),
                value: s.booklets.len().to_string(),
                tone: b::Tone::Neutral,
            },
            b::ReconLine {
                label: "Refused (CAP-001, CAP-004)".into(),
                value: refused.to_string(),
                tone: warn(refused),
            },
            b::ReconLine {
                label: "Not attributed".into(),
                value: unattributed.to_string(),
                tone: warn(unattributed),
            },
        ];
        if let Some(roster) = &s.definition.roster {
            let absent = self.p.absent_students().len();
            reconciliation.push(b::ReconLine {
                label: "Roster without a booklet".into(),
                value: format!("{absent} of {}", roster.len()),
                tone: b::Tone::Neutral,
            });
        }
        let mut activity = Vec::new();
        let ms = |label: &str, v: f64| format!("{label:<12} {v:>7.0} ms");
        activity.push(format!(
            "{} · {} pages · {} threads",
            snapshot_label(&s.snapshot_utc),
            t.pages,
            t.threads
        ));
        // page notices that never reach the queue: pages read turned (REG-002, §8.3) and blank
        // backs dropped (CAP-012, §7.5), with what the engine did
        activity.extend(self.page_notices());
        activity.push(ms("ingest", t.ingest_ms));
        activity.push(ms("page stage", t.page_stage_ms));
        activity.push(ms("batch stage", t.batch_stage_ms));
        activity.push(ms("identity", t.identity_ms));
        activity.push(ms("scoring", t.scoring_ms));
        activity.push(ms("store", t.store_ms));
        activity.push(ms("total", t.total_ms));
        let capture: BTreeSet<String> = s
            .pages
            .iter()
            .filter_map(|p| {
                Some(format!(
                    "{} dpi · {}",
                    p.source_dpi?,
                    format!("{:?}", p.capture?).to_lowercase()
                ))
            })
            .collect();
        let batches = self.batches();
        let turned = s
            .pages
            .iter()
            .filter(|p| {
                p.read
                    .as_ref()
                    .is_some_and(|r| r.exceptions.iter().any(|e| e.code == "REG-002"))
            })
            .count() as u32;
        // pages the engine handled on its own are listed only when there are any (the column
        // must leave room for the activity log at 1280 × 800)
        let key_sheets = s
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::KeySheet)
            .count() as u32;
        let unaligned = s
            .pages
            .iter()
            .filter(|p| alignment_refusal(p).is_some())
            .count() as u32;
        let handled = [
            (
                "Key sheets, not students (§10.1)",
                key_sheets,
                b::Tone::Neutral,
            ),
            (
                "Not read: alignment unverified (REG-003)",
                unaligned,
                b::Tone::Warning,
            ),
            ("Duplicates ignored (CAP-011)", duplicates, b::Tone::Neutral),
            ("Blank backs dropped (CAP-012)", blank, b::Tone::Neutral),
            ("Read turned upright (REG-002)", turned, b::Tone::Neutral),
        ];
        let shown = handled.into_iter().filter(|(_, n, _)| *n > 0);
        for (at, (label, n, tone)) in (3..).zip(shown) {
            reconciliation.insert(
                at,
                b::ReconLine {
                    label: label.into(),
                    value: n.to_string(),
                    tone,
                },
            );
        }
        b::BatchProgress {
            exam: exam.clone(),
            batch_label: format!("Batch {batches} of {batches} · complete"),
            source: "file import".into(),
            device: match capture.len() {
                0 => "capture unknown".into(),
                1 => capture.into_iter().next().unwrap_or_default(),
                n => format!("{n} capture slices"),
            },
            running: false,
            pages_done: s.pages.len() as u32,
            pages_total: s.pages.len() as u32,
            pages_per_sec: pps,
            floor_pages_per_sec: 40.0,
            max_pages_per_sec: 120.0_f64.max(pps * 1.2),
            auto_accepted: auto_share,
            abstention_rate: 1.0 - auto_share,
            first_result: Duration::from_secs_f64(t.first_result_ms.max(0.0) / 1e3),
            remaining: Duration::ZERO,
            in_flight: 0,
            queued: 0,
            stages: vec![
                b::StageCount {
                    stage: "Pages".into(),
                    count: s.pages.len() as u32,
                },
                b::StageCount {
                    stage: "Booklets".into(),
                    count: s.booklets.len() as u32,
                },
                b::StageCount {
                    stage: "Open items".into(),
                    count: s.review.items.iter().filter(|i| i.is_open()).count() as u32,
                },
                b::StageCount {
                    stage: "Quarantined".into(),
                    count: refused + unattributed,
                },
            ],
            exceptions,
            thumbnails,
            reconciliation,
            reconciliation_code: None,
            activity,
            engine: config_line(&self.p),
            workers: t.threads as u32,
            sheets,
            pausable: false,
            append: match &self.read_only {
                Some(why) => b::AppendInfo {
                    allowed: false,
                    note: format!(
                        "Open read-only ({why}): close the other program that has the exam open \
                         to add scans"
                    ),
                },
                None => b::AppendInfo {
                    allowed: true,
                    note: format!(
                        "Every page is graded again with the new ones as batch {}; review \
                         decisions carry over and a disagreeing reread is flagged (SYS-032)",
                        batches + 1
                    ),
                },
            },
        }
    }

    /// Scan batches of the project: `batch-N` of the store (1 when read-only).
    fn batches(&self) -> u32 {
        self.p
            .store()
            .and_then(|st| st.batch_id().strip_prefix("batch-")?.parse().ok())
            .unwrap_or(1)
    }

    /// Activity lines for the page notices of the batch (newest first is not meaningful for a
    /// finished batch, so they follow page order): REG-002 turned pages and CAP-012 blank pages.
    fn page_notices(&self) -> Vec<String> {
        let s = &self.p.session;
        let mut out = Vec::new();
        // the instructor's key sheets: listed as such, never as students (§10.1)
        let ks = &s.key_sheets;
        for k in &ks.sheets {
            let Some(&first) = k.pages.first() else {
                continue;
            };
            let state = match (&k.refusal, k.form) {
                (Some(why), _) => format!("gives no key: {why}"),
                (None, None) => "gives no key: its version is not one of this exam's".into(),
                (None, Some(_)) if ks.superseded => "replaced by a key file".into(),
                (None, Some(f)) if ks.is_confirmed(f) => "key confirmed".into(),
                (None, Some(_)) => "key waits for your confirmation in Setup".into(),
            };
            out.push(format!(
                "KEY {} · key sheet{} · {} · {state}",
                self.serial_of_page(first),
                if k.form_text.is_empty() {
                    String::new()
                } else {
                    format!(" version {}", k.form_text)
                },
                if k.declared {
                    "named"
                } else {
                    "found in the stack"
                },
            ));
        }
        // pages read on no alignment at all (REG-003 ERROR): nothing on them was read, and
        // what to do about it (R10)
        for (i, pg) in s.pages.iter().enumerate() {
            if alignment_refusal(pg).is_some() {
                out.push(alignment_notice(&self.serial_of_page(i)));
            }
        }
        for (i, pg) in s.pages.iter().enumerate() {
            let notices = pg
                .read
                .iter()
                .flat_map(|r| &r.exceptions)
                .filter(|e| e.code == "REG-002")
                .chain(pg.refusal.iter().filter(|e| e.code == "CAP-012"));
            for e in notices {
                // short enough for the activity column: what the engine did with the page
                let d = e.detail.to_lowercase();
                let what = if e.code == "CAP-012" {
                    "blank, dropped"
                } else if d.contains("counter-clockwise") {
                    "portrait, read turned 90° ccw"
                } else if d.contains("clockwise") {
                    "portrait, read turned 90° cw"
                } else if d.contains("180") || d.contains("upside") {
                    "upside down, read turned 180°"
                } else {
                    "read turned upright"
                };
                out.push(format!("{} {} · {what}", e.code, self.serial_of_page(i)));
            }
        }
        out
    }

    fn queue(&self, lane: b::Lane) -> b::ReviewQueue {
        let v = self.p.review_queue_vm(lane_to_app(lane));
        b::ReviewQueue {
            unavailable: None,
            lane,
            lanes: v
                .lanes
                .iter()
                .map(|(l, n)| b::LaneCount {
                    lane: lane_of_app(*l),
                    open: *n,
                })
                .collect(),
            groups: v
                .groups
                .iter()
                .map(|g| b::ExceptionGroup {
                    code: g.code.clone(),
                    title: g.title.clone(),
                    severity: sev(g.severity),
                    items: g
                        .items
                        .iter()
                        .map(|q| b::QueueItem {
                            id: ItemId(q.id),
                            label: q.label.clone(),
                            // sub-reasons are identifiers ("weak_mark"): shown as words
                            detail: q.detail.replace('_', " "),
                            question: q.question,
                            form: q.form.clone(),
                            serial: q.serial.clone(),
                            severity: sev(q.severity),
                            fills: q.fills.clone(),
                            kind: kind(q.kind),
                            deferred: q.deferred,
                        })
                        .collect(),
                })
                .collect(),
            resolved: v.resolved,
            total: v.total,
            median_secs_per_item: None,
            audit_head: v.audit_head.clone(),
            two_person_rule: false,
        }
    }

    fn candidates(&self, booklet: Option<usize>) -> Vec<String> {
        booklet
            .and_then(|bk| self.p.session.booklets.get(bk))
            .map(|b| b.identity.candidates.iter().map(|c| c.0.clone()).collect())
            .unwrap_or_default()
    }

    fn item(&self, id: u64) -> Option<b::ReviewItem> {
        if let Some(hit) = self.items.borrow().get(&id) {
            return hit.clone();
        }
        let made = self.make_item(id);
        self.items.borrow_mut().insert(id, made.clone());
        made
    }

    fn make_item(&self, id: u64) -> Option<b::ReviewItem> {
        let v = self.p.review_item_vm(id)?;
        let s = &self.p.session;
        let it = s.review.item(id)?;
        let def = &s.definition;
        let cands = self.candidates(it.booklet);
        let bubbles: Vec<b::BubbleView> = v
            .bubbles
            .iter()
            .enumerate()
            .map(|(i, x)| b::BubbleView {
                // version bubbles carry the printed letters (the core labels them with the
                // exam's forms, which leaves printed C and D as "2" and "3")
                label: if it.kind == ItemKind::Form {
                    printed(i as u8)
                } else {
                    x.label.clone()
                },
                cx: x.cx,
                cy: x.cy,
                r: x.r,
                fill: x.fill,
                state: mark_state(x.state),
            })
            .collect();
        let mut fills: Vec<f32> = bubbles.iter().map(|x| x.fill).collect();
        fills.sort_by(|a, b| b.total_cmp(a));
        let top = fills.first().copied().unwrap_or(0.0);
        let second = fills.get(1).copied().unwrap_or(0.0);
        let suggestion = v.suggestion.as_ref().and_then(|d| {
            let decision = of_app(d, it.kind, &cands, &def.forms);
            match (it.kind, decision) {
                // a suggested student outside the listed candidates cannot be chosen by key
                (ItemKind::Identity, b::Decision::Candidate(u8::MAX)) => None,
                (ItemKind::Identity, b::Decision::Candidate(n)) => {
                    let p = |i: usize| {
                        s.booklets[it.booklet?]
                            .identity
                            .candidates
                            .get(i)
                            .map(|c| c.1 as f32)
                    };
                    // the nearest roster student of a number that is not on the roster comes
                    // with a posterior near 0: listed as a candidate, never offered as the
                    // one-key default (a wrong student is a personal-data breach, §9)
                    if p(usize::from(n)).unwrap_or(0.0) < MIN_IDENTITY_SUGGESTION {
                        return None;
                    }
                    Some(b::Suggestion {
                        decision,
                        source: b::SuggestionSource::Engine,
                        fill: p(usize::from(n)).unwrap_or(0.0),
                        margin: p(usize::from(n)).unwrap_or(0.0)
                            - p(usize::from(n) + 1).unwrap_or(0.0),
                    })
                }
                (ItemKind::Form, _) => {
                    let inf = it.booklet.and_then(|bk| s.booklets[bk].inference.as_ref());
                    Some(b::Suggestion {
                        decision,
                        source: b::SuggestionSource::Rule("form inference (§10.3)".into()),
                        fill: 0.0,
                        margin: inf.map_or(0.0, |i| i.margin_milli as f32 / 1000.0),
                    })
                }
                _ => Some(b::Suggestion {
                    decision,
                    source: b::SuggestionSource::Engine,
                    fill: top,
                    margin: (top - second).max(0.0),
                }),
            }
        });
        let identity = v.identity.as_ref().map(|idv| {
            let roster = def.roster.as_deref().unwrap_or(&[]);
            let read: Vec<char> = idv.read_id.chars().collect();
            b::IdentityEvidence {
                read_id: idv.read_id.clone(),
                boxes_read: String::new(),
                hypothesis: if it.detail.is_empty() {
                    it.sub_reason.clone().unwrap_or_default().replace('_', " ")
                } else {
                    it.detail.clone()
                },
                candidates: idv
                    .candidates
                    .iter()
                    .map(|(key, posterior)| {
                        let r = roster.iter().find(|r| digits(&r.id) == digits(key));
                        let cand: Vec<char> = digits(key).chars().collect();
                        let (mut differ, mut unread) = (0, 0);
                        for (i, c) in cand.iter().enumerate() {
                            match read.get(i) {
                                Some(x) if x.is_ascii_digit() => differ += usize::from(x != c),
                                _ => unread += 1,
                            }
                        }
                        let mut detail = format!(
                            "{} digit{} differ",
                            differ,
                            if differ == 1 { "" } else { "s" }
                        );
                        if unread > 0 {
                            detail.push_str(&format!(", {unread} unread"));
                        }
                        b::IdentityCandidate {
                            name: r
                                .map_or_else(|| "No roster entry".to_string(), |r| r.name.clone()),
                            name_ar: r.map_or_else(String::new, |r| r.name_ar.clone()),
                            student_id: key.clone(),
                            national_id_masked: String::new(),
                            section: r.map_or_else(String::new, |r| r.section.clone()),
                            campus: String::new(),
                            track: String::new(),
                            seat: String::new(),
                            posterior: *posterior,
                            channels: vec![b::ChannelEvidence {
                                channel: "ID grid".into(),
                                agrees: differ == 0 && unread == 0,
                                detail,
                            }],
                        }
                    })
                    .collect(),
                neighbours: Vec::new(),
            }
        });
        // a page refused on an unverified alignment says what to do (R10), in the core's words
        let unaligned = it.code == "REG-003"
            && it.severity >= AppSeverity::Error
            && it
                .page
                .and_then(|p| s.pages.get(p))
                .is_some_and(|p| alignment_refusal(p).is_some());
        let notice = v.notice.as_ref().map(|n| {
            let mut lines = n.lines.clone();
            if unaligned && !n.headline.contains("rescan") {
                lines.push(("What to do".into(), alignment_remedy().into()));
            }
            b::NoticeEvidence {
                headline: n.headline.clone(),
                lines,
                consequence: n.consequence.clone(),
            }
        });
        // a probable mis-key (SCO-001) is kept only with a reason and a note (§12.2)
        let reasons: Vec<String> = if it.kind == ItemKind::Notice && it.flag.is_none() {
            Vec::new()
        } else {
            omr_app::review::reasons_for(it.kind, it.lane)
                .iter()
                .map(|r| (*r).to_string())
                .collect()
        };
        let summary = match (it.kind, &v.notice) {
            (ItemKind::Form, Some(n)) => format!(
                "Booklet {}, {}. {}. {}",
                v.serial,
                match it.booklet.map(|bk| s.booklets[bk].declared_text.as_str()) {
                    None | Some("") => "the version bubbles are blank".to_string(),
                    Some(t) => format!("the version bubbles read “{t}”"),
                },
                n.lines
                    .iter()
                    .filter(|(k, _)| k.starts_with("LL "))
                    .map(|(k, x)| format!("{} {x}", &k[3..]))
                    .collect::<Vec<_>>()
                    .join(", "),
                v.suggestion
                    .as_ref()
                    .map_or("No suggestion.".to_string(), |d| format!(
                        "Suggestion {}, inferred from the answers.",
                        d.text(&def.option_labels(), &def.forms)
                    ))
            ),
            _ => v.summary.clone(),
        };
        Some(b::ReviewItem {
            id: ItemId(v.id),
            code: v.code.clone(),
            code_title: v.code_title.clone(),
            sub_reason: v.sub_reason.clone(),
            severity: sev(v.severity),
            kind: kind(v.kind),
            question: v.question,
            form: v.form.clone(),
            serial: v.serial.clone(),
            page: v.page,
            dpi: v.dpi,
            crop: crop(&v.crop),
            original: None,
            bubbles,
            threshold: v.threshold,
            band: v.band,
            suggestion,
            other_fills: v.other_fills.clone(),
            typical_full_fill: v.typical_full_fill,
            answered: v.answered,
            summary,
            group: b::GroupConfirm {
                size: v.group.size,
                members: v.group.members.iter().map(|&m| ItemId(m)).collect(),
                allowed: v.group.allowed,
                reason: v.group.reason.clone(),
            },
            current: v
                .current
                .as_ref()
                .map(|d| of_app(d, it.kind, &cands, &def.forms)),
            identity,
            notice,
            reasons: reasons.clone(),
            // the core's own note rule (§14.4: OTHER and SOLO_SELF_APPROVAL say why)
            reasons_needing_note: reasons
                .into_iter()
                .filter(|r| ReviewReason::code(r).validate().is_err())
                .collect(),
        })
    }

    fn history(&self) -> Vec<b::DecisionRecord> {
        let s = &self.p.session;
        s.review
            .history
            .iter()
            .map(|r| {
                let it = s.review.item(r.item);
                let k = it.map_or(ItemKind::Notice, |i| i.kind);
                let cands = self.candidates(it.and_then(|i| i.booklet));
                b::DecisionRecord {
                    seq: r.seq,
                    item: ItemId(r.item),
                    question: it
                        .filter(|i| i.kind == ItemKind::Mark)
                        .and_then(|i| i.question)
                        .map_or(0, |q| u32::from(q) + 1),
                    serial: it
                        .and_then(|i| i.booklet)
                        .map_or_else(String::new, |bk| format!("{:04}", s.booklets[bk].serial)),
                    action: action_of_app(&r.action, k, &cands, &s.definition.forms),
                    reason: r.reason.as_ref().map(|x| x.code.clone()),
                    undone: r.undone,
                }
            })
            .collect()
    }

    fn results(&self, exam: &ExamId) -> Option<b::Results> {
        if let Some(hit) = self.results.borrow().as_ref() {
            return hit.clone();
        }
        let made = self.make_results(exam);
        *self.results.borrow_mut() = Some(made.clone());
        made
    }

    fn make_results(&self, exam: &ExamId) -> Option<b::Results> {
        let v = self.p.results_vm()?;
        let s = &self.p.session;
        let def = &s.definition;
        let analytics = self.p.analytics();
        // flag stability (§12.2): the core's own report when it ran the job, else the job this
        // backend ran for the current score run; "computing…" until one has reported
        let stability = analytics
            .as_ref()
            .and_then(|a| a.stability.clone())
            .or_else(|| self.current_stability());
        let test = analytics.as_ref().and_then(|a| a.tests.first());
        let alpha = v.alpha.map(|a| a.0);
        let sem_points = test.and_then(|t| t.sem);
        // SEM in percent points: SEM = σ·√(1 − α) scales with the score, so the percent SD gives it
        let sem_pct = alpha.map_or(0.0, |a| v.sd_pct * (1.0 - a).max(0.0).sqrt());
        let roster = def.roster.as_ref();
        let absent = v.absent;
        let items = analytics
            .as_ref()
            .map(|a| {
                let n = usize::from(def.questions).max(1);
                a.items
                    .iter()
                    .map(|i| {
                        let mut flags: Vec<&omr_analytics::miskey::ItemFlag> =
                            a.flags.iter().filter(|f| f.item == i.item).collect();
                        flags.sort_by_key(|f| f.rank());
                        b::ItemStats {
                            question: (i.item % n) as u32 + 1,
                            label: if i.label.contains('-') {
                                // a per-form master item, e.g. "B-Q3" → "B-Q03"
                                match i.label.split_once("-Q") {
                                    Some((f, q)) => format!(
                                        "{f}-Q{:02}",
                                        q.parse::<u32>().unwrap_or((i.item % n) as u32 + 1)
                                    ),
                                    None => i.label.clone(),
                                }
                            } else {
                                String::new()
                            },
                            // what the core could not compute stays empty (shown as –)
                            p: i.p,
                            r_ir: i.r_ir,
                            r_ci: i.r_ir_ci,
                            band: i.r_ir.map(b::DiscriminationBand::from_r),
                            option_shares: i
                                .options
                                .iter()
                                .filter(|o| o.option.is_some())
                                .map(|o| o.proportion)
                                .collect(),
                            keyed: i.options.iter().find(|o| o.keyed).and_then(|o| o.option),
                            flags: flags
                                .into_iter()
                                .map(|f| {
                                    let stab = flag_stability(f, stability.as_ref());
                                    b::ItemFlag {
                                        // the rule's statement; the stability account is its
                                        // own column (the core may have appended it)
                                        message: f
                                            .message
                                            .split(" — low confidence (proposed)")
                                            .next()
                                            .unwrap_or_default()
                                            .to_string(),
                                        code: f.rule.code().map(str::to_string),
                                        tone: match f.rank() {
                                            1 => b::Tone::Error,
                                            2..=4 => b::Tone::Warning,
                                            _ => b::Tone::Info,
                                        },
                                        stable: f.confidence
                                            == omr_analytics::miskey::Confidence::Normal
                                            && !matches!(stab, b::FlagStability::Low { .. }),
                                        rank: f.rank(),
                                        stability: stab,
                                    }
                                })
                                .collect(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let readiness = v
            .readiness
            .iter()
            .map(|(ok, text)| {
                let target = if *ok {
                    None
                } else if text.contains("review items open") {
                    Some(b::CheckTarget::Review(None))
                } else if text.contains("Key") || text.contains("key") {
                    Some(b::CheckTarget::Setup)
                } else if text.contains("not attributed") {
                    Some(b::CheckTarget::Review(Some(b::Lane::Capture)))
                } else if text.contains("forms inferred") {
                    Some(b::CheckTarget::Review(Some(b::Lane::Scoring)))
                } else if text.contains("pending") {
                    Some(b::CheckTarget::Review(Some(b::Lane::Marks)))
                } else {
                    Some(b::CheckTarget::Review(None))
                };
                b::ReadinessCheck {
                    tone: if *ok {
                        b::Tone::Success
                    } else {
                        b::Tone::Error
                    },
                    text: singular(text),
                    action: match target {
                        Some(b::CheckTarget::Setup) => "Setup".into(),
                        Some(_) => "Open".into(),
                        None => String::new(),
                    },
                    target,
                }
            })
            .collect();
        Some(b::Results {
            exam: exam.clone(),
            key_label: v.key_label.clone(),
            stats: b::ResultStats {
                students: v.students,
                // 0 without a roster: nobody is known to be absent (§9.2)
                roster: roster.map_or(0, |r| r.len() as u32),
                absent,
                mean_pct: v.mean_pct,
                median_pct: v.median_pct,
                sd_pct: v.sd_pct,
                pass_rate: v.pass_rate,
                alpha,
                alpha_ci: v.alpha.map(|a| a.1),
                sem_points,
                open_errors: s.open_blocking() as u32,
            },
            histogram: b::ScoreHistogram {
                // omr-app bins percentages by tenths; the pass mark is the scale's lowest
                // passing band (60 % on the Saudi scale)
                bin_width: 100.0 / v.histogram.len().max(1) as f64,
                counts: v.histogram.clone(),
                pass_mark: pass_mark(&v.grades),
                mean: v.mean_pct,
                sem: sem_pct,
            },
            scale_label: format!(
                "Saudi grade scale (omr-scoring) · pass mark {} %",
                trim(pass_mark(&v.grades))
            ),
            grades: v
                .grades
                .iter()
                .map(|g| b::GradeBand {
                    letter: g.letter.clone(),
                    letter_ar: g.letter_ar.clone(),
                    lo: g.lo,
                    hi: g.hi,
                    gpa5: None,
                    gpa4: None,
                    count: g.count,
                    fail: g.fail,
                })
                .collect(),
            statuses: Vec::new(),
            items,
            option_labels: v.option_labels.clone(),
            readiness,
            release_blocked: v.release_blocked.clone(),
            // the core's own rule (`omr_app::export::blackboard_csv`): a supplied key, a roster
            // and a score run
            blackboard_blocked: if s.key.as_ref().is_none_or(|k| k.inferred) {
                Some(
                    "The key is inferred from the sheets: grades are pushed only with a supplied \
                     key. Replace it (omr key replace) first"
                        .into(),
                )
            } else if def.roster.is_none() {
                Some(
                    "No roster: results without a roster are never pushed (§16.1). Grade with \
                     --roster"
                        .into(),
                )
            } else if self.p.scoring.as_ref().is_none_or(|sc| sc.run.is_err()) {
                Some("No score run: fix the key or the scoring policy first".into())
            } else if !s.released() {
                Some(
                    "Not released: the Blackboard upload is written when the results are \
                     released (Release results…)"
                        .into(),
                )
            } else {
                None
            },
        })
    }

    fn outcomes(&self, exam: &ExamId) -> Option<b::Outcomes> {
        if let Some(hit) = self.outcomes.borrow().as_ref() {
            return hit.clone();
        }
        let made = self.make_outcomes(exam);
        *self.outcomes.borrow_mut() = Some(made.clone());
        made
    }

    fn make_outcomes(&self, exam: &ExamId) -> Option<b::Outcomes> {
        let o = self.p.outcomes()?;
        let v = self.p.outcomes_vm()?;
        let def = &self.p.session.definition;
        let setup = def.outcomes.as_ref()?;
        let clo_def = |code: &str| setup.clos.iter().find(|c| c.code == code);
        let class_method = match o.policy.class_method {
            omr_outcomes::ClassMethod::Average => b::ClassMethod::Average,
            omr_outcomes::ClassMethod::ThresholdProportion => b::ClassMethod::ThresholdProportion,
            omr_outcomes::ClassMethod::PerQuestion => b::ClassMethod::PerQuestion,
            omr_outcomes::ClassMethod::Levels => b::ClassMethod::Levels,
            omr_outcomes::ClassMethod::Eamu => b::ClassMethod::Eamu,
        };
        let hash = &o.result.provenance.policy_hash;
        let clos = o
            .result
            .clos
            .iter()
            .map(|c| {
                let d = clo_def(&c.clo);
                let mean = c.stats.mean.as_ref().map(|x| exact!(x));
                let prop = c.stats.proportion.as_ref().map(|x| exact!(x));
                b::CloAttainment {
                    code: c.clo.clone(),
                    title: d.map_or_else(String::new, |d| d.title.clone()),
                    title_ar: d.map_or_else(String::new, |d| d.title_ar.clone()),
                    domain: d.map_or(b::NqfDomain::Knowledge, |d| domain(d.domain)),
                    target: exact!(c.target),
                    average: mean.unwrap_or(0.0),
                    average_ci: c
                        .stats
                        .mean_ci
                        .unwrap_or((mean.unwrap_or(0.0), mean.unwrap_or(0.0))),
                    proportion: prop.unwrap_or(0.0),
                    proportion_ci: c
                        .stats
                        .proportion_ci
                        .unwrap_or((prop.unwrap_or(0.0), prop.unwrap_or(0.0))),
                    n: c.stats.n as u32,
                    verdict: match c.attained {
                        Some(true) => b::Verdict::Attained,
                        Some(false) => b::Verdict::NotAttained,
                        None => b::Verdict::InsufficientEvidence,
                    },
                    ci_contains_target: c.indistinguishable,
                    mapping_note: mapping_note(def, &c.clo),
                }
            })
            .collect();
        let tp154_b1 = v
            .tp154_b1
            .iter()
            .map(|r| {
                // rows carry the CLO text, numbered "1.1" (K), "2.1" (S), "3.1" (V)
                let dom = match r[0].split('.').next() {
                    Some("2") => b::NqfDomain::Skills,
                    Some("3") => b::NqfDomain::Values,
                    _ => b::NqfDomain::Knowledge,
                };
                b::Tp154Row {
                    domain: dom,
                    number: r[0].clone(),
                    clo: r[1].clone(),
                    plo: r[2].clone(),
                    methods: r[3].clone(),
                    targeted: r[4].clone(),
                    actual: r[5].clone(),
                    comment: r[6].clone(),
                }
            })
            .collect();
        Some(b::Outcomes {
            exam: exam.clone(),
            policy: b::PolicyView {
                class_method,
                tau: v.tau,
                target: v.target,
                mapping_basis: "every mapping of the exam definition (no approval step)".into(),
                verdict_basis: match o.policy.verdict_basis {
                    omr_outcomes::policy::VerdictBasis::Point => "point".into(),
                    omr_outcomes::policy::VerdictBasis::CiLower => "ci_lower".into(),
                },
                version: format!(
                    "{} v{} · sha384 {}…",
                    o.policy.name,
                    o.policy.version,
                    hash.get(..8).unwrap_or(hash)
                ),
            },
            clos,
            tp154_b1,
            classification: v.classification.clone(),
        })
    }
}

/// A path for the status bar: `~` for the home folder, and the last folders only when long.
/// Split on either separator and joined with the platform's, so Windows paths keep their tail.
fn short_path(p: &Path) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let mut text = p.display().to_string();
    if let Some(home) = std::env::home_dir()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        text = format!("~{sep}{}", rest.display());
    }
    if text.chars().count() <= 48 {
        return text;
    }
    let parts: Vec<&str> = text
        .split(std::path::is_separator)
        .filter(|s| !s.is_empty())
        .collect();
    let mut tail = String::new();
    for part in parts.iter().rev() {
        let next = if tail.is_empty() {
            (*part).to_string()
        } else {
            format!("{part}{sep}{tail}")
        };
        if next.chars().count() > 44 {
            break;
        }
        tail = next;
    }
    if tail.is_empty() {
        // One last component longer than the budget: keep its end.
        let last = parts.last().copied().unwrap_or("");
        let n = last.chars().count();
        tail = last.chars().skip(n.saturating_sub(43)).collect();
    }
    format!("…{sep}{tail}")
}

fn short_page(page_id: &str) -> String {
    // "pg-<16 hex>-001" → "3fa2·1": content hash prefix and page within the file
    let mut parts = page_id.split('-');
    let _ = parts.next();
    let hash = parts.next().unwrap_or("");
    let page = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(1);
    format!("{}·{page}", hash.get(..4).unwrap_or(hash))
}

fn engine_line(p: &ExamProject) -> String {
    format!(
        "omr {} · config {}",
        p.session.manifest.app_version,
        short_hash(&p.session.manifest.config_sha384)
    )
}

/// The recognition configuration a batch ran with (status bar of the Scan screen).
fn config_line(p: &ExamProject) -> String {
    format!(
        "engine config {}",
        short_hash(&p.session.manifest.config_sha384)
    )
}

fn short_hash(h: &str) -> String {
    format!(
        "{}…{}",
        h.get(..4).unwrap_or(""),
        h.get(h.len().saturating_sub(4)..).unwrap_or("")
    )
}

fn summary_of(v: &vm::ExamSummary, snapshot_utc: &str) -> b::ExamSummary {
    b::ExamSummary {
        id: ExamId(v.id.clone()),
        course_code: v.course_code.clone(),
        course_title: v.course_title.clone(),
        course_title_ar: v.course_title_ar.clone(),
        assessment: v.assessment.clone(),
        term: b::TermLabel {
            label: v.term.clone(),
            ..b::TermLabel::default()
        },
        date: v.date.clone(),
        date_hijri: String::new(),
        versions: v.versions.clone(),
        status: status(v.status),
        sheets_scanned: v.sheets_scanned,
        sheets_expected: v.sheets_expected,
        open_exceptions: counts(v.open_exceptions),
        key_status: v.key_status.clone(),
        mapped_items: v.mapped_items,
        // omr-app has no approval step: every mapping of the definition is in force
        mapping_approval: false,
        total_items: v.total_items,
        roster_count: v.roster_count,
        sections: v.sections.clone(),
        last_scan: snapshot_label(snapshot_utc),
        batch_id: v.batch_id.clone(),
    }
}

/// Dashboard row of a grading run that has no project yet.
fn job_summary(j: &Job, d: &ExamDefinition) -> b::ExamSummary {
    let snap = j.tracker.snapshot();
    b::ExamSummary {
        id: j.exam.clone(),
        course_code: d.course_code.clone(),
        course_title: d.course_title.clone(),
        course_title_ar: d.course_title_ar.clone(),
        assessment: d.title.clone(),
        term: b::TermLabel {
            label: d.term.clone(),
            ..b::TermLabel::default()
        },
        date: d.date.clone(),
        date_hijri: String::new(),
        versions: d.forms.clone(),
        status: if j.outcome.is_some() {
            b::ExamStatus::Setup
        } else {
            b::ExamStatus::Scanning
        },
        sheets_scanned: snap.pages_done,
        sheets_expected: d
            .roster
            .as_ref()
            .map_or(snap.pages_total, |r| r.len() as u32),
        open_exceptions: b::SeverityCounts::default(),
        key_status: match &d.key {
            omr_app::KeySource::None => "No key".into(),
            omr_app::KeySource::Inferred => "Key · INFERRED (after grading)".into(),
            omr_app::KeySource::Provided { .. } => "Key v1 · draft".into(),
        },
        mapped_items: d.outcomes.as_ref().map_or(0, |o| {
            o.mapping
                .iter()
                .map(|m| m.question)
                .collect::<BTreeSet<_>>()
                .len() as u32
        }),
        mapping_approval: false,
        total_items: u32::from(d.questions),
        roster_count: d.roster.as_ref().map_or(0, |r| r.len() as u32),
        sections: d
            .roster
            .iter()
            .flatten()
            .map(|r| r.section.clone())
            .filter(|s| !s.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        last_scan: "now".into(),
        batch_id: String::new(),
    }
}

/// Live batch view of a grading run, from the omr-app progress tracker.
fn job_batch(j: &Job, opened: Option<&Opened>) -> b::BatchProgress {
    let v = j.tracker.snapshot();
    let thumbnails = v
        .thumbnails
        .iter()
        .take(THUMBS)
        .map(|(page_id, st)| {
            // once the project is open, a page shows its booklet serial and its scan
            let page =
                opened.and_then(|o| o.p.session.pages.iter().position(|p| &p.page_id == page_id));
            b::Thumbnail {
                serial: match (opened, page) {
                    (Some(o), Some(p)) => o.serial_of_page(p),
                    _ => short_page(page_id),
                },
                state: match st {
                    vm::ThumbState::Auto => b::ThumbState::Auto,
                    vm::ThumbState::Review(c, s) => b::ThumbState::Review {
                        code: c.clone(),
                        severity: sev(*s),
                    },
                    vm::ThumbState::Rejected(c, s) => b::ThumbState::Rejected {
                        code: c.clone(),
                        severity: sev(*s),
                    },
                    vm::ThumbState::Reading => b::ThumbState::Reading,
                    vm::ThumbState::Queued => b::ThumbState::Queued,
                },
                image: match (opened, page) {
                    (Some(o), Some(p)) => o.thumbnail(p),
                    _ => None,
                },
            }
        })
        .collect();
    let mut activity = j.activity.clone();
    activity.extend(v.activity.iter().cloned());
    let failed = matches!(j.outcome, Some(Err(_)));
    let n = match j.kind {
        JobKind::Grade { .. } => 1,
        JobKind::Append { batch, .. } => batch,
    };
    b::BatchProgress {
        exam: j.exam.clone(),
        batch_label: match (&j.outcome, &j.kind) {
            (None, JobKind::Append { .. }) => format!("Batch {n} of {n} · adding scans"),
            (None, _) => format!("Batch {n} of {n}"),
            (Some(Ok(_)), _) => format!("Batch {n} of {n} · complete"),
            (Some(Err(_)), _) => format!("Batch {n} of {n} · failed"),
        },
        source: v.source.clone(),
        device: format!("{} workers", v.workers),
        running: j.outcome.is_none(),
        pages_done: v.pages_done,
        pages_total: v.pages_total,
        pages_per_sec: v.pages_per_sec,
        floor_pages_per_sec: v.floor_pages_per_sec,
        max_pages_per_sec: v.max_pages_per_sec.max(v.pages_per_sec * 1.2),
        auto_accepted: v.auto_accepted,
        abstention_rate: v.abstention_rate,
        first_result: v.first_result,
        remaining: v.remaining,
        in_flight: if failed { 0 } else { v.in_flight },
        queued: if failed { 0 } else { v.queued },
        stages: v
            .stages
            .iter()
            .map(|(s, n)| b::StageCount {
                stage: match s.as_str() {
                    "read" => "Read".into(),
                    "queued" => "Queued".into(),
                    o => o.to_string(),
                },
                count: *n,
            })
            .collect(),
        exceptions: v
            .exceptions
            .iter()
            .map(|(c, s, n)| b::CodeCount {
                code: c.clone(),
                title: title_of(c),
                severity: sev(*s),
                count: *n,
            })
            .collect(),
        thumbnails,
        reconciliation: vec![b::ReconLine {
            label: "Pages read".into(),
            value: format!("{} of {}", v.pages_done, v.pages_total),
            tone: b::Tone::Neutral,
        }],
        reconciliation_code: None,
        activity,
        engine: opened.map_or_else(
            || format!("engine omr {}", env!("CARGO_PKG_VERSION")),
            |o| config_line(&o.p),
        ),
        workers: v.workers,
        sheets: j.sheet_counts(),
        pausable: false,
        append: b::AppendInfo {
            allowed: false,
            note: if j.running() {
                "Grading is running: add more scans when this batch is complete".into()
            } else {
                "The exam could not be opened; see the activity log".into()
            },
        },
    }
}

// ------------------------------------------------------------------------------------ trait

// ------------------------------------------------------------ Remark migration (§3.5, §23.7)

/// A Remark import held for review until it is applied: to a new exam (the New-exam form) or
/// to a graded exam's key (Setup).
struct HeldImport {
    /// The workbook files as given.
    files: Vec<PathBuf>,
    imp: omr_app::remark::RemarkImport,
    /// How the points were settled when the instructor gave some ("" when none).
    points_note: String,
    /// The student numbers of the export's rows (key sheets left out): whether the export is
    /// this exam's is checked against its students before it may replace its key.
    ids: BTreeSet<String>,
}

/// The student numbers of the workbooks' rows (digits only; key sheets and unread IDs left
/// out).
fn row_ids(wbs: &[omr_app::remark::RemarkWorkbook]) -> BTreeSet<String> {
    wbs.iter()
        .flat_map(|w| w.rows.iter())
        .filter(|r| {
            r.key_sheet.is_none()
                && r.student_id
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ' ' || c == '-')
        })
        .map(|r| digits(&r.student_id))
        .filter(|d| !d.is_empty())
        .collect()
}

fn file_label(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The message of an omr-app error without its kind prefix (a refusal's reason).
fn app_message(e: &AppError) -> String {
    match e {
        AppError::Definition(m)
        | AppError::Input(m)
        | AppError::Store(m)
        | AppError::Scoring(m)
        | AppError::Review(m)
        | AppError::NotFound(m)
        | AppError::Internal(m) => m.clone(),
        AppError::Io(e) => e.to_string(),
        AppError::Cancelled => e.to_string(),
    }
}

/// Points given, as text: "version A: question 6 → 0.75, question 10 → 0.75".
fn given_text(g: &omr_app::remark::GivenPoints) -> String {
    g.iter()
        .filter_map(|(form, col)| {
            let qs: Vec<String> = col
                .iter()
                .enumerate()
                .filter_map(|(q, v)| v.as_ref().map(|v| format!("question {} → {v}", q + 1)))
                .collect();
            (!qs.is_empty()).then(|| {
                if form.is_empty() {
                    qs.join(", ")
                } else {
                    format!("version {form}: {}", qs.join(", "))
                }
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `1-8,11,13-14` from 1-based questions.
fn question_ranges(qs: &[u16]) -> String {
    let mut v = qs.to_vec();
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

/// A refusal of the import or the parallel run in this window's words: the core's reason
/// (which names the files, rows or questions; its command-line flag named as this window's
/// way) and what to do here (R10).
fn remark_refusal(
    e: &AppError,
    suggestion: Option<&omr_app::remark::GivenPoints>,
    compare: bool,
) -> b::RemarkRefusal {
    let message = app_message(e)
        .replace(
            "give the points with --points FILE",
            if compare {
                "give the points (this exam's, or a points file read by the import in Setup)"
            } else {
                "give the points in a points file"
            },
        )
        .replace("with --points FILE", "with a points file");
    let suggestion = suggestion.map(given_text).unwrap_or_default();
    let points =
        message.contains("do not determine the points") || message.contains("do not reproduce");
    let remedy = if !suggestion.is_empty() {
        if compare {
            "Choose Use the suggested points if they are right (your decision: they are never \
             assumed), or Use this exam's points"
        } else {
            "Choose Use the suggested points if they are right (your decision: the import never \
             assumes them), or write the points in a file (question,points_A) and give it in \
             Points file"
        }
    } else if points && compare {
        "Choose Use this exam's points (checked against every Remark total), or check the \
         grading in Remark and export again"
    } else if points {
        "Write the points in a file (question,points or question,points_A,points_B; a question \
         left empty is solved from the totals), give it in Points file and read again"
    } else {
        "Give the graded Excel workbooks Remark exported for this exam (.xlsx: the Advanced \
         Grade reports with the Student Response Report), ; between several. Remark's .rmx and \
         .aky files are never opened"
    };
    b::RemarkRefusal {
        message,
        remedy: remedy.into(),
        suggestion,
    }
}

fn no_workbooks() -> b::RemarkRefusal {
    b::RemarkRefusal {
        message: "No Remark workbook given".into(),
        remedy: "Type the graded Excel workbooks Remark exported for this exam (.xlsx), ; \
                 between several"
            .into(),
        suggestion: String::new(),
    }
}

/// The points the request gives (and how, for the summary): none, the import's suggestion
/// (the instructor's explicit choice), or the exam's own points.
fn given_points(
    wbs: &[omr_app::remark::RemarkWorkbook],
    points: &b::RemarkPoints,
    def: Option<&ExamDefinition>,
) -> Result<(omr_app::remark::GivenPoints, String), AppError> {
    use omr_app::remark::{self, GivenPoints};
    match points {
        b::RemarkPoints::FromWorkbooks | b::RemarkPoints::File(_) => {
            Ok((GivenPoints::new(), String::new()))
        }
        b::RemarkPoints::Suggested => {
            let s = remark::suggest_points(wbs, &GivenPoints::new())?;
            if !s.values().any(|c| c.iter().any(Option::is_some)) {
                return Err(AppError::Input(
                    "the workbooks leave no points to suggest: every version's points follow \
                     from its learning objectives or totals, or none is fully determined"
                        .into(),
                ));
            }
            let text = given_text(&s);
            Ok((
                s,
                format!(
                    "given by you as suggested ({text}): Remark's totals alone do not determine \
                     them; the values are those another version, fully determined, leaves, or a \
                     learning objective's points split equally"
                ),
            ))
        }
        b::RemarkPoints::Exam => {
            let def = def.ok_or_else(|| {
                AppError::Input("there is no exam whose points could be used".into())
            })?;
            let g: GivenPoints = def
                .forms
                .iter()
                .cloned()
                .zip(def.points_by_form())
                .map(|(f, p)| (f, p.into_iter().map(Some).collect()))
                .collect();
            Ok((
                g,
                "this exam's points, checked against every Remark total".into(),
            ))
        }
    }
}

/// Reads the workbooks of `req` and imports them (omr-app does the reading, R4); a refusal
/// with what to do, and the points the import suggests when the totals leave some open.
fn read_import(
    req: &b::RemarkRequest,
    def: Option<&ExamDefinition>,
) -> Result<HeldImport, b::RemarkRefusal> {
    use omr_app::remark::{self, GivenPoints};
    if req.workbooks.is_empty() {
        return Err(no_workbooks());
    }
    if let b::RemarkPoints::File(f) = &req.points {
        let (wbs, imp) = remark::import_files(&req.workbooks, Some(f))
            .map_err(|e| remark_refusal(&e, None, false))?;
        return Ok(HeldImport {
            files: req.workbooks.clone(),
            imp,
            ids: row_ids(&wbs),
            points_note: format!(
                "the points given in {} are kept; the others come from the workbooks",
                file_label(f)
            ),
        });
    }
    let wbs =
        remark::read_workbook_files(&req.workbooks).map_err(|e| remark_refusal(&e, None, false))?;
    let (given, points_note) =
        given_points(&wbs, &req.points, def).map_err(|e| remark_refusal(&e, None, false))?;
    match remark::import(&wbs, &given) {
        Ok(imp) => Ok(HeldImport {
            files: req.workbooks.clone(),
            imp,
            points_note,
            ids: row_ids(&wbs),
        }),
        Err(e) => {
            let suggestion = (req.points == b::RemarkPoints::FromWorkbooks)
                .then(|| remark::suggest_points(&wbs, &GivenPoints::new()).ok())
                .flatten()
                .filter(|g| g.values().any(|c| c.iter().any(Option::is_some)));
            Err(remark_refusal(&e, suggestion.as_ref(), false))
        }
    }
}

fn key_sheet_why(k: &omr_app::remark::KeySheetRowInfo) -> String {
    use omr_app::remark::KeySheetKind;
    match k.kind {
        KeySheetKind::RepeatedDigitId => format!(
            "student ID of one repeated digit{}",
            if k.answers_equal_key && k.full_marks {
                "; answers equal the key, full marks"
            } else {
                ""
            }
        ),
        KeySheetKind::AnswersEqualKey => "answers equal the key with full marks: a key sheet, \
                                          or a student with every answer right (check)"
            .into(),
    }
}

/// The import as the instructor reviews it.
fn import_view(
    h: &HeldImport,
    applies: String,
    can_apply: bool,
    objectives: bool,
) -> b::RemarkImportView {
    use omr_scoring::exact_text;
    let imp = &h.imp;
    let versions = imp
        .versions
        .iter()
        .map(|v| {
            let missing = v.key.iter().filter(|k| k.trim().is_empty()).count();
            let several = v.key.iter().filter(|k| k.contains(['|', '&', ','])).count();
            let distinct: BTreeSet<String> = v.points.iter().map(exact_text).collect();
            b::RemarkVersion {
                form: v.form.clone(),
                remark_version: v.remark_version.clone(),
                workbooks: v.workbooks.clone(),
                key: v.key.clone(),
                key_counts: format!(
                    "{} {} · {} with several answers · {} missing",
                    v.key.len() - missing,
                    if v.key.len() - missing == 1 {
                        "entry"
                    } else {
                        "entries"
                    },
                    several,
                    missing
                ),
                points: v.points.iter().map(exact_text).collect(),
                points_source: v.points_source.describe().to_string(),
                total: exact_text(&v.total_possible),
                distinct: distinct.into_iter().collect::<Vec<_>>().join(", "),
                students: v.students,
                key_sheet_rows: v.key_sheet_rows,
                reproduced: v.totals_reproduced,
                rows_checked: v.rows_checked,
                group_points_checked: v.group_points_checked,
                objectives: v
                    .groups
                    .iter()
                    .map(|g| b::RemarkObjective {
                        code: g.name.clone(),
                        label: g.label.clone(),
                        questions: question_ranges(&g.questions),
                        points: exact_text(&g.possible),
                    })
                    .collect(),
            }
        })
        .collect();
    b::RemarkImportView {
        files: h.files.iter().map(|f| file_label(f)).collect(),
        refusal: None,
        questions: imp.questions,
        versions,
        workbooks: imp
            .workbooks
            .iter()
            .map(|w| b::RemarkWorkbook {
                file: w.file.clone(),
                digest: short_hash(&w.sha384),
                form: w.form.clone(),
                students: w.students,
                key_sheets: w.key_sheets,
                footer_rows: w.footer_rows,
                objectives: w.groups,
            })
            .collect(),
        key_sheets: imp
            .key_sheet_rows
            .iter()
            .map(|k| b::RemarkKeySheetRow {
                workbook: k.file.clone(),
                row: k.row,
                masked_id: k.masked_id.clone(),
                why: key_sheet_why(k),
            })
            .collect(),
        notes: imp
            .notes
            .iter()
            .map(|n| omr_app::export::mask_numbers(n))
            .collect(),
        points_note: h.points_note.clone(),
        applies,
        can_apply,
        objectives_applied: objectives,
    }
}

fn refused_import(files: &[PathBuf], r: b::RemarkRefusal) -> b::RemarkImportView {
    b::RemarkImportView {
        files: files.iter().map(|f| file_label(f)).collect(),
        refusal: Some(r),
        ..Default::default()
    }
}

/// What applying the import to a new exam does.
fn new_exam_applies(imp: &omr_app::remark::RemarkImport) -> String {
    let objectives: BTreeSet<&str> = imp
        .versions
        .iter()
        .flat_map(|v| v.groups.iter().map(|g| g.name.as_str()))
        .collect();
    format!(
        "Use this import fills the form with {} questions and versions {}; the exam is graded \
         with this key and these points (exact, per version){}",
        imp.questions,
        imp.forms().join(", "),
        if objectives.is_empty() {
            "; the export has no learning objectives (give a CLO mapping file if you have one)"
                .to_string()
        } else {
            format!(
                ", and the {} learning objective(s) become CLO mappings with weight 1 flagged \
                 imported, needs review (proposed: the TP-154 tables stay a draft until they are \
                 approved)",
                objectives.len()
            )
        }
    )
}

/// The activity-log lines of a new exam created from a Remark import (counts only).
fn remark_activity(imp: &omr_app::remark::RemarkImport) -> Vec<String> {
    let mut out = vec![format!(
        "Remark import · {} workbook(s) · versions {} · {} questions · key and exact points per \
         version",
        imp.workbooks.len(),
        imp.forms().join(", "),
        imp.questions
    )];
    let objectives: BTreeSet<&str> = imp
        .versions
        .iter()
        .flat_map(|v| v.groups.iter().map(|g| g.name.as_str()))
        .collect();
    if !objectives.is_empty() {
        out.push(format!(
            "Remark import · {} learning objective(s) as CLO mappings, weight 1, imported, \
             needs review",
            objectives.len()
        ));
    }
    if !imp.key_sheet_rows.is_empty() {
        out.push(format!(
            "Remark import · {} key-sheet row(s) graded as students in Remark: excluded",
            imp.key_sheet_rows.len()
        ));
    }
    out
}

/// Whether the import fits a graded exam, and what applying it does there.
fn setup_applies(h: &HeldImport, p: &ExamProject) -> (String, bool) {
    let imp = &h.imp;
    let def = &p.session.definition;
    if imp.questions != def.questions {
        return (
            format!(
                "Not applicable: the Remark export has {} questions and this exam {}; import the \
                 export of this exam, or create a new exam with it (New exam → Import from Remark \
                 export…)",
                imp.questions, def.questions
            ),
            false,
        );
    }
    let forms = imp.forms();
    if forms.iter().collect::<BTreeSet<_>>() != def.forms.iter().collect::<BTreeSet<_>>() {
        return (
            format!(
                "Not applicable: the Remark export gives version(s) {} and this exam has {}; \
                 import every version's workbook of this exam",
                forms.join(", "),
                def.forms.join(", ")
            ),
            false,
        );
    }
    // the export is this exam's: some of its students are this exam's (a Remark export
    // carries no exam identity, only its rows)
    let ours: BTreeSet<String> = p
        .session
        .booklets
        .iter()
        .filter_map(|b| b.student().map(digits))
        .chain(def.roster.iter().flatten().map(|r| digits(&r.id)))
        .filter(|d| !d.is_empty())
        .collect();
    if !h.ids.is_empty() && !ours.is_empty() && h.ids.is_disjoint(&ours) {
        return (
            format!(
                "Not applicable: none of the export's {} student numbers is a student of this \
                 exam, so it is another exam's or another section's export; import the export \
                 of this exam's sheets",
                h.ids.len()
            ),
            false,
        );
    }
    // a graded exam's points are fixed with its stored form map (omr-store keeps it
    // immutable): only a key with the same points can replace its key
    let differ = points_differences(imp, def);
    if p.scoring.is_some() && !differ.is_empty() {
        return (
            format!(
                "Not applicable here: this exam was graded with other points ({}{}) and a graded \
                 exam's points are fixed once its form map is stored. Create the exam from this \
                 import (New exam → Import from Remark export…) to grade with Remark's points, \
                 or compare this exam with Remark in Results",
                differ
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
                if differ.len() > 3 {
                    format!("; {} questions in all", differ.len())
                } else {
                    String::new()
                }
            ),
            false,
        );
    }
    let next = p.session.key.as_ref().map_or(1, |k| k.version + 1);
    (
        format!(
            "Apply puts this key in force as key version {next}: the exam is rescored and the \
             change audited with the workbooks' SHA-384"
        ),
        true,
    )
}

/// The questions whose imported points differ from the exam's ("version A question 9: 1 here,
/// 0.5 in Remark").
fn points_differences(imp: &omr_app::remark::RemarkImport, def: &ExamDefinition) -> Vec<String> {
    let num = |s: &str| s.trim().parse::<f64>().ok();
    let mut out = Vec::new();
    for (form, ours) in def.forms.iter().zip(def.points_by_form()) {
        let Some(theirs) = imp.points_of(form) else {
            continue;
        };
        for (q, (a, b)) in ours.iter().zip(theirs).enumerate() {
            let b = omr_scoring::exact_text(b);
            let same = match (num(a), num(&b)) {
                (Some(x), Some(y)) => (x - y).abs() < 1e-9,
                _ => a.trim() == b,
            };
            if !same {
                out.push(format!(
                    "version {form} question {}: {a} here, {b} in Remark",
                    q + 1
                ));
            }
        }
    }
    out
}

fn stats_text(
    mean: Option<f64>,
    sd: Option<f64>,
    max: Option<f64>,
    max_pct: Option<f64>,
    kr20: Option<f64>,
    alpha: Option<f64>,
) -> String {
    let f = |x: Option<f64>, d: usize| x.map_or_else(|| "–".to_string(), |v| format!("{v:.d$}"));
    format!(
        "mean {}, SD {}, top {}{}, KR-20 {}, α {}",
        f(mean, 2),
        f(sd, 2),
        f(max, 2),
        max_pct.map_or_else(String::new, |p| format!(" ({p:.0} %)")),
        f(kr20, 3),
        f(alpha, 3)
    )
}

/// The parallel run as the instructor adjudicates it (student numbers masked on screen; the
/// saved report has them).
fn parallel_view(
    run: &omr_app::parallel::ParallelRun,
    files: &[PathBuf],
    points: &str,
) -> b::ParallelRunView {
    use omr_app::export::mask_numbers;
    use omr_app::parallel::Agreement;
    use omr_scoring::exact_text;
    let c = &run.counts;
    let mut differences: Vec<b::ParallelDifference> = run
        .disagreements
        .iter()
        .map(|d| b::ParallelDifference {
            booklet: d.page.clone(),
            student: mask_numbers(&d.student),
            question: u32::from(d.question),
            remark: d.remark.clone(),
            ours: d.ours.clone(),
            status: d.status.clone(),
            agreement: match d.agreement {
                Agreement::Differ => b::ParallelAgreement::Differ,
                Agreement::DifferPending => b::ParallelAgreement::DifferPending,
                _ => b::ParallelAgreement::AgreePending,
            },
            findings: d.findings.clone(),
            crop: d.crop.as_ref().map(marked_crop),
        })
        .collect();
    // settled differences first, then pending ones that differ, then pending ones that agree
    differences.sort_by_key(|d| match d.agreement {
        b::ParallelAgreement::Differ => 0,
        b::ParallelAgreement::DifferPending => 1,
        b::ParallelAgreement::AgreePending => 2,
    });
    let pts = |x: &omr_scoring::Exact| exact_text(x);
    b::ParallelRunView {
        banner: omr_app::parallel::RESTRICTED_BANNER.to_string(),
        files: files.iter().map(|f| file_label(f)).collect(),
        refusal: None,
        points: if points.is_empty() {
            let sources: BTreeSet<&str> = run
                .import
                .versions
                .iter()
                .map(|v| v.points_source.describe())
                .collect();
            if sources.len() == 1 {
                sources.into_iter().collect::<String>()
            } else {
                run.import
                    .versions
                    .iter()
                    .map(|v| format!("version {}: {}", v.form, v.points_source.describe()))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        } else {
            points.to_string()
        },
        counts: b::ParallelCounts {
            booklets: c.booklets,
            remark_rows: c.remark_rows,
            out_of_scope_rows: c.out_of_scope_rows,
            key_sheet_rows: c.key_sheet_rows,
            paired_by_id: c.paired_by_id,
            paired_by_partial_id: c.paired_by_partial_id,
            paired_by_answers: c.paired_by_answers,
            questions: c.questions,
            agree: c.agree,
            agree_pending: c.agree_pending,
            differ: c.differ,
            differ_pending: c.differ_pending,
            totals_settled: c.totals_settled,
            totals_settled_equal: c.totals_settled_equal,
            totals_provisional: c.totals_provisional,
            totals_provisional_equal_if_remark: c.totals_provisional_equal_if_remark,
            totals_not_graded: c.totals_not_graded,
            answers_only_equal: c.answers_only_equal,
            identity: c.identity.values().sum(),
            key_differences: c.key_differences,
            points_differences: c.points_differences,
            maximum_differences: c.maximum_differences,
            not_queued: c.not_queued,
            held_in_review: c.held_in_review,
        },
        hold: run.hold_text(),
        points_differences: run
            .points_differences
            .iter()
            .map(|(f, q, o, r)| format!("version {f} question {q}: {o} here, {r} in Remark"))
            .collect(),
        maximum_differences: run
            .maximum_differences
            .iter()
            .map(|(f, o, r)| format!("version {f}: {o} here, {r} in Remark"))
            .collect(),
        differences,
        identity: run
            .identity
            .iter()
            .map(|i| b::ParallelIdentity {
                kind: i.kind.describe().to_string(),
                booklet: i.booklet.clone(),
                ours: {
                    let id = if i.our_id.is_empty() {
                        "no student number".to_string()
                    } else {
                        mask_numbers(&i.our_id)
                    };
                    if i.our_flags.is_empty() {
                        id
                    } else {
                        format!("{id} · {}", i.our_flags.join(", "))
                    }
                },
                remark: match (i.remark_row.is_empty(), i.remark_id.is_empty()) {
                    (true, _) => "no row".to_string(),
                    (false, true) => format!("{} · no student number", i.remark_row),
                    (false, false) => {
                        format!("{} · {}", i.remark_row, mask_numbers(&i.remark_id))
                    }
                },
                detail: mask_numbers(&i.detail),
            })
            .collect(),
        totals: run
            .students
            .iter()
            .filter(|s| s.remark.is_some() && (!s.settled || !s.equals_remark()))
            .map(|s| b::ParallelTotal {
                booklet: s.label.clone(),
                student: mask_numbers(&s.our_id),
                ours: format!("{} / {}", pts(&s.ours), pts(&s.max)),
                remark: s.remark.as_ref().map(pts).unwrap_or_default(),
                pending: question_ranges(&s.pending),
                if_remark: if s.settled {
                    String::new()
                } else {
                    pts(&s.ours_if_remark)
                },
                grades: format!("{} · Remark {}", s.grade_ours, s.grade_remark),
            })
            .collect(),
        key_sheets: run
            .key_sheets
            .iter()
            .map(|k| b::ParallelKeySheets {
                workbook: k.workbook.clone(),
                rows: k.rows.len(),
                reported: k.reported.as_ref().map_or_else(
                    || "not in the workbook".to_string(),
                    |r| stats_text(r.mean, r.sd, r.max, None, r.kr20, r.alpha),
                ),
                with: stats_text(
                    k.with.mean,
                    k.with.sd,
                    k.with.max,
                    k.with.max_percent,
                    k.with.kr20,
                    k.with.alpha,
                ),
                without: stats_text(
                    k.without.mean,
                    k.without.sd,
                    k.without.max,
                    k.without.max_percent,
                    k.without.kr20,
                    k.without.alpha,
                ),
            })
            .collect(),
        workbooks: run
            .workbooks
            .iter()
            .map(|w| b::ParallelWorkbook {
                file: w.file.clone(),
                form: w.form.clone(),
                rows: w.rows,
                key_sheets: w.key_sheets,
                paired: w.paired,
                in_scope: w.in_scope,
            })
            .collect(),
        key_differences: run
            .key_differences
            .iter()
            .map(|(f, q, _, _)| format!("version {f} question {q}"))
            .collect(),
    }
}

/// Runs the parallel run of `req` on the opened exam (omr-app reads the workbooks and the
/// stored originals for the crops).
fn run_parallel(
    o: &Opened,
    req: &b::RemarkRequest,
) -> Result<(omr_app::parallel::ParallelRun, String), b::RemarkRefusal> {
    use omr_app::remark::{self, GivenPoints};
    if req.workbooks.is_empty() {
        return Err(no_workbooks());
    }
    if matches!(req.points, b::RemarkPoints::File(_)) {
        return Err(b::RemarkRefusal {
            message: "A points file is read by the import, not by the parallel run".into(),
            remedy: "Use this exam's points, or import the export with its points file in Setup \
                     first"
                .into(),
            suggestion: String::new(),
        });
    }
    let wbs =
        remark::read_workbook_files(&req.workbooks).map_err(|e| remark_refusal(&e, None, true))?;
    let def = &o.p.session.definition;
    let (given, note) =
        given_points(&wbs, &req.points, Some(def)).map_err(|e| remark_refusal(&e, None, true))?;
    let mut handwritten = Vec::new();
    for f in &req.handwritten {
        let bytes = std::fs::read(f).map_err(|e| b::RemarkRefusal {
            message: format!("{}: {e}", f.display()),
            remedy: "Check the path of the handwritten-ID file (file,page,student_id)".into(),
            suggestion: String::new(),
        })?;
        handwritten.extend(
            omr_app::parallel::read_handwritten_csv(&bytes)
                .map_err(|e| remark_refusal(&e, None, true))?,
        );
    }
    let opts = omr_app::parallel::CompareOptions {
        given_points: given,
        handwritten,
        crops: true,
    };
    match o.p.compare_remark(&wbs, &opts) {
        Ok(run) => Ok((run, note)),
        Err(e) => {
            let suggestion = (req.points == b::RemarkPoints::FromWorkbooks)
                .then(|| remark::suggest_points(&wbs, &GivenPoints::new()).ok())
                .flatten()
                .filter(|g| g.values().any(|c| c.iter().any(Option::is_some)));
            Err(remark_refusal(&e, suggestion.as_ref(), true))
        }
    }
}

impl Backend for AppBackend {
    fn dashboard(&self) -> b::Dashboard {
        let mut exams: Vec<b::ExamSummary> = Vec::new();
        if let Some(j) = &self.job
            && let JobKind::Grade { definition } = &j.kind
            && !self.entries.iter().any(|e| e.id == j.exam)
        {
            exams.push(job_summary(j, definition));
        }
        // sheets of the runs graded in the last 7 days and today (UTC days)
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| (d.as_secs() / 86_400) as i64);
        let (mut week, mut day) = (0u32, 0u32);
        let opened = self.opened.borrow();
        for e in &self.entries {
            let mut s = match opened.get(&e.id) {
                Some(o) => o.summary(Some(e)),
                None => {
                    let mut s = summary_of(&e.summary, &e.snapshot_utc);
                    s.id = e.id.clone();
                    s
                }
            };
            if self.busy(&e.id) {
                s.status = b::ExamStatus::Scanning;
            }
            if let Some(d) = epoch_day(&e.snapshot_utc).filter(|d| *d <= today) {
                if today - d < 7 {
                    week += s.sheets_scanned;
                }
                if d == today {
                    day += s.sheets_scanned;
                }
            }
            exams.push(s);
        }
        drop(opened);
        // throughput of the most recent run (pages through the page stage per second)
        let throughput = self
            .entries
            .iter()
            .max_by(|a, b| a.snapshot_utc.cmp(&b.snapshot_utc))
            .filter(|e| e.page_stage_ms > 0.0)
            .map(|e| e.pages as f64 / (e.page_stage_ms / 1e3));
        let open: u32 = exams.iter().map(|e| e.open_exceptions.total()).sum();
        let errors: u32 = exams.iter().map(|e| e.open_exceptions.blocking()).sum();
        let term = exams.first().map(|e| e.term.clone()).unwrap_or_default();
        let initials: String = self
            .operator
            .trim_start_matches("principal:")
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .take(2)
            .filter_map(|w| w.chars().next())
            .flat_map(char::to_uppercase)
            .collect();
        let engine = {
            let opened = self.opened.borrow();
            self.default
                .as_ref()
                .and_then(|d| opened.get(d))
                .map_or_else(
                    || format!("omr {}", env!("CARGO_PKG_VERSION")),
                    |o| engine_line(&o.p),
                )
        };
        b::Dashboard {
            user: b::UserInfo {
                name: self.operator.trim_start_matches("principal:").to_string(),
                role: "Operator".into(),
                initials,
            },
            term,
            stats: b::DashboardStats {
                sheets_this_week: week,
                sheets_today: day,
                open_exceptions: open,
                open_errors: errors,
                median_review_secs: None,
                throughput_pages_per_sec: throughput,
            },
            exams,
            engine,
            workspace: self
                .workspace
                .as_ref()
                .map_or_else(|| "no workspace".to_string(), |w| short_path(w)),
            data_note: "Scans, names and student numbers are read from the exam folders of this \
                        workspace and stay on this computer (PDPL); every review action is an \
                        audited event in the exam's project store."
                .into(),
        }
    }

    fn default_exam(&self) -> Option<ExamId> {
        self.default.clone()
    }

    fn exam_setup(&self, exam: &ExamId) -> Option<b::ExamSetup> {
        self.with(exam, |o| o.setup(exam))
    }

    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), BackendError> {
        self.with_mut(exam, |o| {
            o.p.set_key(version, question, answer).map_err(|e| match e {
                AppError::Review(m) => BackendError::Invalid(format!(
                    "{m}. Type an option letter, two letters such as A,C, * to void, or nothing \
                     to clear"
                )),
                e => app_err(e),
            })
        })
    }

    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), BackendError> {
        let q = u16::try_from(question)
            .map_err(|_| BackendError::NotFound(format!("Question {question} does not exist")))?;
        self.with_mut(exam, |o| {
            o.p.set_mapping_weight(q, clo, weight).map_err(|e| match e {
                AppError::Review(m) => BackendError::Invalid(format!(
                    "{m}: type a weight above 0 and at most 1, or clear the cell"
                )),
                e => app_err(e),
            })
        })
    }

    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), BackendError> {
        let q = u16::try_from(question)
            .map_err(|_| BackendError::NotFound(format!("Question {question} does not exist")))?;
        self.with_mut(exam, |o| {
            o.p.set_mapping_note(q, clo, note).map_err(|e| match e {
                AppError::NotFound(m) => BackendError::NotFound(format!(
                    "{m}: give the cell a weight first, then its note"
                )),
                e => app_err(e),
            })
        })
    }

    fn batch_progress(&self, exam: &ExamId) -> Option<b::BatchProgress> {
        if let Some(j) = self.job.as_ref().filter(|j| &j.exam == exam) {
            let opened = self.opened.borrow();
            return Some(match (opened.get(exam), &j.outcome) {
                // finished and open: the whole batch from the session (codes, reconciliation,
                // timings), after the run's own activity lines (a refused added batch leaves
                // the exam as it was, with the reason on top)
                (Some(o), Some(_)) => {
                    let mut b = o.batch(exam);
                    let mut activity = j.activity.clone();
                    activity.append(&mut b.activity);
                    b.activity = activity;
                    b
                }
                (o, _) => job_batch(j, o),
            });
        }
        self.with(exam, |o| o.batch(exam))
    }

    fn review_queue(&self, exam: &ExamId, lane: b::Lane) -> b::ReviewQueue {
        // an exam without a queue says why (never "no open items" while it is being graded)
        let unavailable = || {
            if self
                .job
                .as_ref()
                .is_some_and(|j| &j.exam == exam && j.running())
            {
                Some(
                    "The exam is being graded: its review queue appears when the Scan screen \
                     says the batch is complete"
                        .to_string(),
                )
            } else if let Some(why) = self.failed.borrow().get(exam) {
                Some(why.clone())
            } else {
                Some("No graded project is open for this exam".to_string())
            }
        };
        self.with(exam, |o| o.queue(lane))
            .unwrap_or_else(|| b::ReviewQueue {
                unavailable: unavailable(),
                lane,
                lanes: b::Lane::ALL
                    .iter()
                    .map(|&l| b::LaneCount { lane: l, open: 0 })
                    .collect(),
                groups: Vec::new(),
                resolved: 0,
                total: 0,
                median_secs_per_item: None,
                audit_head: String::new(),
                two_person_rule: false,
            })
    }

    fn review_item(&self, exam: &ExamId, item: ItemId) -> Option<b::ReviewItem> {
        self.with(exam, |o| o.item(item.0)).flatten()
    }

    fn apply(
        &mut self,
        exam: &ExamId,
        item: ItemId,
        action: b::ReviewAction,
    ) -> Result<b::ApplyOutcome, BackendError> {
        self.apply_with_reason(exam, item, action, None)
    }

    fn needs_reason(&self, exam: &ExamId, item: ItemId, decision: b::Decision) -> bool {
        self.with(exam, |o| {
            let Some(it) = o.p.session.review.item(item.0) else {
                return false;
            };
            to_app(decision, it.kind, &o.p.session.definition.forms)
                .is_ok_and(|d| o.p.overrides_engine(item.0, &d))
        })
        .unwrap_or(false)
    }

    fn needs_reason_for_student(&self, exam: &ExamId, item: ItemId, student_id: &str) -> bool {
        self.with(exam, |o| {
            o.p.overrides_engine(item.0, &AppDecision::Student(student_id.to_string()))
        })
        .unwrap_or(false)
    }

    fn apply_with_reason(
        &mut self,
        exam: &ExamId,
        item: ItemId,
        action: b::ReviewAction,
        reason: Option<String>,
    ) -> Result<b::ApplyOutcome, BackendError> {
        self.with_mut(exam, |o| {
            let k =
                o.p.session
                    .review
                    .item(item.0)
                    .map(|i| i.kind)
                    .ok_or_else(|| {
                        BackendError::NotFound(format!(
                            "Review item {} does not exist in this exam; reopen the exam",
                            item.0
                        ))
                    })?;
            let forms = o.p.session.definition.forms.clone();
            // the note travels with the reason (the audit event keeps only its hash, §14.4)
            let reason = reason.map(|code| match &action {
                b::ReviewAction::Resolve { note, .. }
                | b::ReviewAction::SetStudent { note, .. } => ReviewReason::with_note(&code, note),
                _ => ReviewReason::code(&code),
            });
            let a = match action {
                b::ReviewAction::Resolve {
                    decision,
                    accepted_suggestion,
                    note,
                } => Action::Resolve {
                    decision: to_app(decision, k, &forms)?,
                    accepted_suggestion,
                    note,
                },
                b::ReviewAction::ApplyToGroup { decision } => Action::ApplyToGroup {
                    decision: to_app(decision, k, &forms)?,
                },
                b::ReviewAction::SetStudent { student_id, note } => {
                    if k != ItemKind::Identity {
                        return Err(BackendError::Invalid(
                            "A student number decides identity items only".into(),
                        ));
                    }
                    Action::Resolve {
                        decision: AppDecision::Student(student_id),
                        accepted_suggestion: false,
                        note,
                    }
                }
                b::ReviewAction::Defer => Action::Defer,
                b::ReviewAction::Escalate => Action::Escalate,
                b::ReviewAction::AddToRescan => Action::AddToRescan,
            };
            let out = o.p.apply_with_reason(item.0, a, reason).map_err(app_err)?;
            Ok(b::ApplyOutcome {
                affected: out.affected.into_iter().map(ItemId).collect(),
                next: out.next.map(ItemId),
            })
        })
    }

    fn undo(&mut self, exam: &ExamId) -> Result<Option<ItemId>, BackendError> {
        self.with_mut(exam, |o| o.p.undo().map(|r| r.map(ItemId)).map_err(app_err))
    }

    fn review_history(&self, exam: &ExamId) -> Vec<b::DecisionRecord> {
        self.with(exam, Opened::history).unwrap_or_default()
    }

    fn results(&self, exam: &ExamId) -> Option<b::Results> {
        self.ensure_stability(exam);
        self.with(exam, |o| o.results(exam)).flatten()
    }

    fn outcomes(&self, exam: &ExamId) -> Option<b::Outcomes> {
        self.with(exam, |o| o.outcomes(exam)).flatten()
    }

    fn export(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        self.with_mut(exam, |o| {
            let out = o.dir.parent().map(Path::to_path_buf).ok_or_else(|| {
                BackendError::Denied(format!(
                    "{} has no parent folder to export into; move the project into a folder",
                    o.dir.display()
                ))
            })?;
            let sum = o.p.export(&out).map_err(app_err)?;
            let tp154: Vec<&str> = sum
                .files
                .iter()
                .map(String::as_str)
                .filter(|f| f.starts_with("tp154"))
                .collect();
            let blackboard = if sum
                .files
                .iter()
                .any(|f| f == omr_app::export::BLACKBOARD_FILE)
            {
                format!(
                    "; the Blackboard upload is {}",
                    omr_app::export::BLACKBOARD_FILE
                )
            } else {
                String::new()
            };
            Ok(format!(
                "Wrote {} files to {}{}{blackboard}",
                sum.files.len(),
                short_path(&out),
                if tp154.is_empty() {
                    " (no TP-154 tables: the exam has no CLO mapping)".to_string()
                } else {
                    format!(" (TP-154: {})", tp154.join(", "))
                }
            ))
        })
    }

    fn export_blackboard(
        &mut self,
        exam: &ExamId,
        column_id: Option<&str>,
    ) -> Result<String, BackendError> {
        let column_id = column_id.map(str::trim).filter(|c| !c.is_empty());
        self.with_mut(exam, |o| {
            let out = o.dir.parent().map(Path::to_path_buf).ok_or_else(|| {
                BackendError::Denied(format!(
                    "{} has no parent folder to export into; move the project into a folder",
                    o.dir.display()
                ))
            })?;
            let path = out.join(omr_app::export::BLACKBOARD_FILE);
            o.p.export_blackboard(&path, column_id).map_err(app_err)?;
            Ok(format!(
                "Wrote {}: upload it in Blackboard's Grade Center (Work offline → Upload){}",
                short_path(&path),
                match column_id {
                    Some(c) => format!(", into column {c}"),
                    None => " as a new column".to_string(),
                }
            ))
        })
    }

    fn add_scans(&mut self, exam: &ExamId, inputs: &[PathBuf]) -> Result<String, BackendError> {
        self.start_append(exam, inputs)
    }

    fn can_create_exam(&self) -> bool {
        self.workspace.is_some() && self.spawn.is_some() && !self.grading_in_progress()
    }

    fn create_exam(&mut self, form: &b::NewExam) -> Result<(ExamId, String), BackendError> {
        self.start_new_exam(form)
    }

    fn can_replace_key(&self, exam: &ExamId) -> bool {
        self.with(exam, |o| o.read_only.is_none()).unwrap_or(false)
    }

    fn replace_key(&mut self, exam: &ExamId, path: &Path) -> Result<String, BackendError> {
        self.with_mut(exam, |o| {
            // read by omr-app (rule R4), with every points column (`points`, `points_A` …)
            let kf = omr_app::define::read_key_path_full(path, &o.p.session.definition)
                .map_err(app_err)?;
            let has_points = kf.points.all.is_some() || !kf.points.per_form.is_empty();
            o.p.replace_key_points(kf.forms, has_points.then_some(&kf.points), None)
                .map_err(app_err)?;
            let v = o.p.session.key.as_ref().map_or(0, |k| k.version);
            Ok(match o.p.session.release_blocked() {
                Some(why) => format!(
                    "Key version {v} in force: the exam was rescored. Release is still blocked: \
                     {why}"
                ),
                None => format!("Key version {v} in force: the exam was rescored"),
            })
        })
    }

    fn key_sheets(&self, exam: &ExamId) -> Option<b::KeySheetSummary> {
        self.with(exam, Opened::key_summary).flatten()
    }

    fn hold(&self, exam: &ExamId) -> Option<String> {
        self.with(exam, |o| {
            o.p.session.key_unconfirmed().then(|| {
                "The key read from the key sheets is not confirmed: nothing is scored until you \
                 confirm it in Setup (Key from answer sheets)"
                    .to_string()
            })
        })
        .flatten()
    }

    fn read_key_sheets(
        &mut self,
        exam: &ExamId,
        inputs: &[PathBuf],
    ) -> Result<String, BackendError> {
        if inputs.is_empty() {
            return Err(BackendError::Invalid(
                "Give the key-sheet files: scans or photos of the instructor's filled answer \
                 sheets (student number bubbled all 0 or all 9, the version bubbled), ; between \
                 several"
                    .into(),
            ));
        }
        self.with_mut(exam, |o| {
            let def = o.p.session.definition.clone();
            // read by omr-app with the exam's own recognizer (R2: key-blind; R4: its I/O and
            // thread pool), in memory: nothing is written to the exam yet
            let p = omr_app::keysheet::read_key_sheets(&def, inputs, 0).map_err(app_err)?;
            let files: Vec<String> = inputs
                .iter()
                .map(|i| {
                    i.file_name()
                        .map_or_else(|| i.display().to_string(), |n| n.to_string_lossy().into())
                })
                .collect();
            let Some(sum) = p.key_sheet_summary(false) else {
                return Err(BackendError::Invalid(format!(
                    "No page of {} could be read as a key sheet: check that the files are scans \
                     or photos of this exam's answer sheet",
                    files.join(", ")
                )));
            };
            let msg = if sum.versions.is_empty() {
                format!(
                    "No key could be read from {}: {}",
                    files.join(", "),
                    sum.unusable
                        .iter()
                        .filter_map(|k| k.refusal.as_deref())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            } else {
                let open: usize = sum.versions.iter().map(|v| v.open).sum();
                format!(
                    "Read the key of version {} from {}: {}; nothing changes until you confirm",
                    sum.versions
                        .iter()
                        .map(|v| v.version.as_str())
                        .collect::<Vec<_>>()
                        .join(" and "),
                    files.join(", "),
                    match open {
                        0 => "every row read clearly, check them against the sheet".to_string(),
                        1 => "1 row needs your decision".to_string(),
                        n => format!("{n} rows need your decision"),
                    }
                )
            };
            o.preview = Some(Preview {
                p,
                files,
                paths: inputs.to_vec(),
            });
            Ok(msg)
        })
    }

    fn confirm_key_sheets(
        &mut self,
        exam: &ExamId,
        version: Option<usize>,
        decisions: &[b::KeyRowDecision],
    ) -> Result<String, BackendError> {
        let operator = self.operator.clone();
        self.with_mut(exam, |o| {
            let summary = o.key_summary().ok_or_else(|| {
                BackendError::Invalid(
                    "This exam has no key sheets to confirm: read them with Read key from answer \
                     sheet…"
                        .into(),
                )
            })?;
            let forms = o.p.session.definition.forms.clone();
            let targets: Vec<&b::KeySheetVersion> = summary
                .versions
                .iter()
                .filter(|v| v.confirmed.is_none() && version.is_none_or(|f| v.form == f))
                .collect();
            if targets.is_empty() {
                return Err(BackendError::Invalid(match version {
                    Some(f) => format!(
                        "Version {} has no key from key sheets waiting for confirmation",
                        forms.get(f).map_or("?", String::as_str)
                    ),
                    None if summary.superseded => {
                        "A key file replaced the key sheets: there is nothing to confirm".into()
                    }
                    None => "Every version read from the key sheets is confirmed already".into(),
                }));
            }
            // every row that waits for the instructor needs a decision first (named here in
            // the desktop's words; the core names the command line's)
            let missing: Vec<String> = targets
                .iter()
                .flat_map(|v| {
                    v.rows
                        .iter()
                        .filter(|r| r.status.open())
                        .filter(|r| {
                            !decisions
                                .iter()
                                .any(|d| d.version == v.form && d.question == r.question)
                        })
                        .map(|r| {
                            format!(
                                "version {} question {} ({})",
                                v.version,
                                r.question,
                                r.status.label()
                            )
                        })
                })
                .collect();
            if !missing.is_empty() {
                return Err(BackendError::Invalid(format!(
                    "Decide {} first: choose the answer in its row (an option, Accept both, or \
                     Void), then confirm",
                    missing.join(", ")
                )));
            }
            let decs: Vec<omr_app::model::KeyRowDecision> = decisions
                .iter()
                .filter(|d| targets.iter().any(|v| v.form == d.version) && d.question > 0)
                .map(|d| omr_app::model::KeyRowDecision {
                    form: d.version,
                    question: d.question as usize - 1,
                    answer: d.answer.clone(),
                })
                .collect();
            let Some(pv) = o.preview.as_mut() else {
                // the exam's own key sheets: the core records the confirmation and, with the
                // last version, publishes the key and scores the exam
                let r =
                    o.p.confirm_key_sheets(version, decs, None)
                        .map_err(app_err)?;
                return Ok(confirmed_message(&r));
            };
            let r =
                pv.p.confirm_key_sheets(version, decs, None)
                    .map_err(app_err)?;
            if r.held {
                return Ok(format!(
                    "Version {} checked; {} still {} for your confirmation. The exam's key \
                     changes when every version read is confirmed",
                    r.confirmed.join(" and "),
                    r.waiting.join(" and "),
                    if r.waiting.len() == 1 {
                        "waits"
                    } else {
                        "wait"
                    }
                ));
            }
            // every version read is confirmed in the preview: the key sheets join the exam as
            // named key sheets (their originals stored with the scans, the exam graded again
            // with them) and the exam's own confirmation publishes the key version that names
            // them and the instructor (§10.1, C7), with the decisions taken in the preview
            let def = o.p.session.definition.clone();
            let covered: Vec<usize> = pv.p.session.key_sheets.forms();
            for (fi, f) in def.forms.iter().enumerate() {
                if covered.contains(&fi) {
                    continue;
                }
                if !o.p.session.key.as_ref().is_some_and(|k| !k.inferred) {
                    return Err(BackendError::Invalid(format!(
                        "Version {f} has no key sheet among these files and the exam has no \
                         confirmed key for it (its key is {}): read version {f}'s key sheet \
                         too, or replace the whole key from a key file",
                        if o.p.session.key.is_some() {
                            "INFERRED"
                        } else {
                            "missing"
                        }
                    )));
                }
            }
            let decided: Vec<omr_app::model::KeyRowDecision> =
                pv.p.session.key_sheets.decisions.clone();
            let paths = pv.paths.clone();
            let files = pv.files.join(", ");
            let opts = GradeOptions {
                threads: 0,
                project_dir: None,
                snapshot_utc: None,
                operator: operator.clone(),
                max_pages: None,
                cancel: Default::default(),
            };
            o.p.add_key_sheets(&paths, &opts, &|_| {})
                .map_err(app_err)?;
            o.preview = None;
            // read again with the exam's own baselines: a row that now needs a decision is
            // decided in Setup's Key from answer sheets (the exam stays held until then)
            let r = match o.p.confirm_key_sheets(None, decided, None) {
                Ok(r) => r,
                Err(e) => {
                    return Ok(format!(
                        "The key sheets from {files} were added to the exam and wait for your \
                         confirmation in Key from answer sheets: {e}"
                    ));
                }
            };
            if r.held {
                return Ok(format!(
                    "The key sheets from {files} were added to the exam; {} still {} for your \
                     confirmation in Key from answer sheets",
                    r.waiting.join(" and "),
                    if r.waiting.len() == 1 {
                        "waits"
                    } else {
                        "wait"
                    }
                ));
            }
            let v = o.p.session.key.as_ref().map_or(0, |k| k.version);
            let mut m = format!(
                "Key version {v} in force: read from {files} (stored with the exam's scans), \
                 confirmed by you; the exam was graded again with them"
            );
            if !r.warnings.is_empty() {
                m.push_str(&format!(". Check: {}", r.warnings.join("; ")));
            }
            if let Some(why) = o.p.session.release_blocked() {
                m.push_str(&format!(". Release is still blocked: {why}"));
            }
            Ok(m)
        })
    }

    fn discard_key_sheets(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        self.with_mut(exam, |o| match o.preview.take() {
            Some(pv) => Ok(format!(
                "Put aside the key read from {}: the exam's key is unchanged",
                pv.files.join(", ")
            )),
            None => Err(BackendError::Invalid(
                "No key-sheet files were read for this exam".into(),
            )),
        })
    }

    fn can_import_remark(&self, exam: Option<&ExamId>) -> bool {
        match exam {
            None => self.workspace.is_some(),
            Some(e) => self.with(e, |o| o.read_only.is_none()).unwrap_or(false),
        }
    }

    fn read_remark_import(
        &mut self,
        exam: Option<&ExamId>,
        request: &b::RemarkRequest,
    ) -> Result<b::RemarkImportView, BackendError> {
        let Some(exam) = exam else {
            if self.workspace.is_none() {
                return Err(BackendError::Denied(
                    "No exams folder is open: start omr-desktop with --workspace DIR (or without \
                     flags, for ~/OMR Exams)"
                        .into(),
                ));
            }
            self.remark_new = None;
            return Ok(match read_import(request, None) {
                Ok(h) => {
                    let view = import_view(&h, new_exam_applies(&h.imp), true, true);
                    self.remark_new = Some(h);
                    view
                }
                Err(r) => refused_import(&request.workbooks, r),
            });
        };
        self.with_mut(exam, |o| {
            o.remark = None;
            let def = o.p.session.definition.clone();
            Ok(match read_import(request, Some(&def)) {
                Ok(h) => {
                    let (applies, can) = setup_applies(&h, &o.p);
                    let view = import_view(&h, applies, can, false);
                    o.remark = Some(h);
                    view
                }
                Err(r) => refused_import(&request.workbooks, r),
            })
        })
    }

    fn discard_remark_import(&mut self, exam: Option<&ExamId>) {
        match exam {
            None => self.remark_new = None,
            Some(e) => {
                if let Some(o) = self.opened.borrow_mut().get_mut(e) {
                    o.remark = None;
                }
            }
        }
    }

    fn apply_remark_import(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let operator = self.operator.clone();
        self.with_mut(exam, |o| {
            let Some(h) = o.remark.as_ref() else {
                return Err(BackendError::Invalid(
                    "No Remark import is waiting: read the export with Import from Remark \
                     export…, check the summary, then Apply"
                        .into(),
                ));
            };
            let (why, fits) = setup_applies(h, &o.p);
            if !fits {
                return Err(BackendError::Invalid(why));
            }
            let forms = h.imp.form_keys();
            // the same points are kept as the exam stores them (a graded exam's are fixed)
            let points = (o.p.scoring.is_none()
                || !points_differences(&h.imp, &o.p.session.definition).is_empty())
            .then(|| h.imp.key_points());
            let files = h
                .imp
                .workbooks
                .iter()
                .map(|w| format!("{} (SHA-384 {})", w.file, w.sha384.get(..16).unwrap_or("")))
                .collect::<Vec<_>>()
                .join(", ");
            let note = format!(
                "key and points imported from Remark Office OMR's graded export: {files}; {}by \
                 {operator}",
                if h.points_note.is_empty() {
                    String::new()
                } else {
                    format!("points {}; ", h.points_note)
                }
            );
            let versions = h.imp.forms().join(", ");
            let objectives = h.imp.versions.iter().any(|v| !v.groups.is_empty());
            let reason = omr_app::review::ReviewReason::with_note("ADJUDICATION", &note);
            o.p.replace_key_points(forms, points.as_ref(), Some(reason))
                .map_err(app_err)?;
            o.remark = None;
            let v = o.p.session.key.as_ref().map_or(0, |k| k.version);
            let mut m = format!(
                "Key version {v} in force from the Remark import (versions {versions}, exact \
                 points per version): the exam was rescored and the change recorded with the \
                 workbooks' SHA-384"
            );
            if objectives {
                m.push_str(
                    ". The learning objectives were not applied: a graded exam's CLO mapping \
                     cannot be replaced from here yet",
                );
            }
            if let Some(why) = o.p.session.release_blocked() {
                m.push_str(&format!(". Release is still blocked: {why}"));
            }
            Ok(m)
        })
    }

    fn compare_remark(
        &mut self,
        exam: &ExamId,
        request: &b::RemarkRequest,
    ) -> Result<b::ParallelRunView, BackendError> {
        if self.busy(exam) {
            return Err(BackendError::Denied(format!(
                "Scans are being added to {exam}: compare when the Scan screen says the batch is \
                 complete"
            )));
        }
        let r = self.with(exam, |o| {
            run_parallel(o, request)
                .map(|(run, note)| parallel_view(&run, &request.workbooks, &note))
        });
        let Some(r) = r else {
            return Err(match self.failed.borrow().get(exam) {
                Some(why) => BackendError::Denied(why.clone()),
                None => BackendError::NotFound(format!(
                    "Exam {exam} is not open yet: wait for grading to finish, or open its folder \
                     with --project"
                )),
            });
        };
        if let Some(o) = self.opened.borrow_mut().get_mut(exam) {
            o.parallel = r.is_ok().then(|| request.clone());
        }
        Ok(r.unwrap_or_else(|refusal| b::ParallelRunView {
            banner: omr_app::parallel::RESTRICTED_BANNER.to_string(),
            files: request.workbooks.iter().map(|f| file_label(f)).collect(),
            refusal: Some(refusal),
            ..Default::default()
        }))
    }

    fn save_parallel_run(&mut self, exam: &ExamId, dir: &Path) -> Result<String, BackendError> {
        if dir.as_os_str().is_empty() {
            return Err(BackendError::Invalid(
                "Type the folder for the Restricted report (inside the university's storage; \
                 sync folders are refused)"
                    .into(),
            ));
        }
        self.with_mut(exam, |o| {
            let req = o.parallel.clone().ok_or_else(|| {
                BackendError::Invalid(
                    "Run Compare with Remark export… first: the report is written from it".into(),
                )
            })?;
            // rerun on the exam as it stands, so the report never lags a review decision
            let (run, _) = run_parallel(o, &req)
                .map_err(|r| BackendError::Invalid(format!("{}. {}", r.message, r.remedy)))?;
            let sum = o.p.export_parallel_run(&run, dir).map_err(app_err)?;
            Ok(format!(
                "Wrote the Restricted parallel-run report ({} files, signed manifest) to {}; \
                 its head-to-head figures stay inside the university (Q-08). {}",
                sum.files.len(),
                short_path(dir),
                run.hold_text()
            ))
        })
    }

    fn approve_mappings(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let approver = self.operator.clone();
        self.with_mut(exam, |o| {
            let n =
                o.p.approve_mappings(
                    &approver,
                    "approved in the desktop app after review in Setup",
                )
                .map_err(app_err)?;
            Ok(format!(
                "Approved {n} imported CLO mapping row(s) as {approver} (an audited approval): \
                 the TP-154 tables leave the draft watermark at the next export"
            ))
        })
    }

    fn replace_clos(&mut self, exam: &ExamId, path: &Path) -> Result<String, BackendError> {
        if path.as_os_str().is_empty() {
            return Err(BackendError::Invalid(
                "Type the path of the CLO file (clo,domain,title,plo,questions,weight,note)".into(),
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| {
            BackendError::Invalid(format!(
                "{}: {e}; check the path of the CLO file",
                short_path(path)
            ))
        })?;
        let name = file_label(path);
        self.with_mut(exam, |o| {
            let (clos, rows) = o.p.replace_outcomes(&bytes, &name).map_err(app_err)?;
            Ok(format!(
                "Replaced the CLO mapping from {name}: {clos} CLO(s), {rows} row(s) (audited); \
                 the outcomes recompute from the same scores"
            ))
        })
    }

    fn queue_parallel_differences(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        self.with_mut(exam, |o| {
            let req = o.parallel.clone().ok_or_else(|| {
                BackendError::Invalid(
                    "Run Compare with Remark export… first: the differences are queued from it"
                        .into(),
                )
            })?;
            // rerun on the exam as it stands, so a decision taken since is never queued again
            let (mut run, _) = run_parallel(o, &req)
                .map_err(|r| BackendError::Invalid(format!("{}. {}", r.message, r.remedy)))?;
            let n = o.p.queue_parallel_differences(&mut run).map_err(app_err)?;
            Ok(if n == 0 {
                format!("Nothing new to queue. {}", run.hold_text())
            } else {
                format!(
                    "Queued {n} parallel-run difference(s) in Review (Marks lane): each holds \
                     release until you decide it; keeping this exam's reading is Enter, \
                     Remark's or another answer needs a reason"
                )
            })
        })
    }

    fn parallel_run_dir(&self, exam: &ExamId) -> Option<PathBuf> {
        self.with(exam, |o| {
            o.dir.parent().map(|d| d.join("parallel-run-remark"))
        })
        .flatten()
    }

    fn release(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        self.with_mut(exam, |o| {
            let out = o.dir.parent().map(Path::to_path_buf).ok_or_else(|| {
                BackendError::Denied(format!(
                    "{} has no parent folder to export into; move the project into a folder",
                    o.dir.display()
                ))
            })?;
            o.p.release(None).map_err(app_err)?;
            let sum = o.p.export(&out).map_err(app_err)?;
            let r =
                o.p.session
                    .release
                    .clone()
                    .unwrap_or_else(|| unreachable!("released"));
            let bb = if sum
                .files
                .iter()
                .any(|f| f == omr_app::export::BLACKBOARD_FILE)
            {
                format!(
                    "; the Blackboard upload is {}",
                    short_path(&out.join(omr_app::export::BLACKBOARD_FILE))
                )
            } else {
                "; no Blackboard upload (an exam without a roster is never pushed)".to_string()
            };
            Ok(format!(
                "Results released (key version {}, score run {}…) and exported to {}{bb}",
                r.key_version,
                &r.score_run[..12.min(r.score_run.len())],
                short_path(&out)
            ))
        })
    }

    fn pump(&mut self) -> bool {
        let analysed = self.drain_stability(false);
        self.drain_job() || analysed
    }

    fn settle(&mut self) -> bool {
        self.drain_stability(true)
    }

    fn take_reloaded(&mut self) -> Vec<ExamId> {
        std::mem::take(&mut self.reloaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omr_app::exam::TemplateChoice;

    #[test]
    fn long_paths_keep_their_last_folders_on_every_os() {
        let sep = std::path::MAIN_SEPARATOR;
        let p: PathBuf = [
            "a-workspace-folder-with-a-long-name",
            "CS 522 Final exam, second semester",
            "exports",
            "blackboard_grade_center.csv",
        ]
        .iter()
        .collect();
        let s = short_path(&p);
        assert!(s.starts_with(&format!("…{sep}")), "{s}");
        assert!(
            s.ends_with(&format!("exports{sep}blackboard_grade_center.csv")),
            "{s}"
        );
        assert!(s.chars().count() <= 46, "{s}");
        let short: PathBuf = ["exams", "midterm.csv"].iter().collect();
        assert_eq!(short_path(&short), short.display().to_string());
        let one_long: PathBuf = ["x".repeat(60)].iter().collect();
        let s = short_path(&one_long);
        assert!(
            s.ends_with(&"x".repeat(43)) && s.chars().count() <= 46,
            "{s}"
        );
    }

    #[test]
    fn dates_and_labels() {
        assert_eq!(epoch_day("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_day("2026-10-17T09:00:00Z"), Some(20_743));
        assert_eq!(
            snapshot_label("2026-10-12T09:05:00Z"),
            "2026-10-12 09:05 UTC"
        );
        assert_eq!(short_page("pg-3fa29c0011aa22bb-002"), "3fa2·2");
        assert_eq!(digits(" 9 26-1 "), "9261");
        assert_eq!(parse_weight("1/4"), 0.25);
        assert_eq!(singular("1 forms inferred"), "1 form inferred");
        assert_eq!(
            singular("1 ERROR/BLOCKER review items open"),
            "1 ERROR/BLOCKER review item open"
        );
        assert_eq!(singular("11 provisional totals"), "11 provisional totals");
        assert_eq!(parse_weight("0.5"), 0.5);
    }

    #[test]
    fn decisions_map_both_ways() {
        let c = vec!["9261034417".to_string(), "9261034477".to_string()];
        // a definition may list its forms in another order than printed
        let f = vec!["B".to_string(), "A".to_string()];
        assert_eq!(
            to_app(b::Decision::Candidate(1), ItemKind::Identity, &f).unwrap(),
            AppDecision::Option(1)
        );
        assert_eq!(
            to_app(b::Decision::Multi, ItemKind::Mark, &f).unwrap(),
            AppDecision::Multi(Vec::new())
        );
        // printed A is the definition's second form
        assert_eq!(
            to_app(b::Decision::Option(0), ItemKind::Form, &f).unwrap(),
            AppDecision::Form(1)
        );
        let err = to_app(b::Decision::Option(2), ItemKind::Form, &f).unwrap_err();
        assert!(err.to_string().contains("Version C"), "{err}");
        assert!(to_app(b::Decision::Duplicate, ItemKind::Identity, &f).is_err());
        assert_eq!(
            of_app(
                &AppDecision::Student("9261034477".into()),
                ItemKind::Identity,
                &c,
                &f
            ),
            b::Decision::Candidate(1)
        );
        assert_eq!(
            of_app(
                &AppDecision::Student("9000000000".into()),
                ItemKind::Identity,
                &c,
                &f
            ),
            b::Decision::Candidate(u8::MAX)
        );
        assert_eq!(
            of_app(&AppDecision::Option(2), ItemKind::Identity, &c, &f),
            b::Decision::Candidate(2)
        );
        assert_eq!(
            of_app(&AppDecision::Form(0), ItemKind::Form, &c, &f),
            b::Decision::Option(1)
        );
    }

    #[test]
    fn an_unverified_alignment_is_named_with_its_remedy() {
        use omr_model::{Exception, Scope, Severity};
        let refused = Exception::must("REG-003", Severity::Error, Scope::Page).with_detail(
            "alignment with the blank form not verified (a bubble lattice can match one row or \
             column away); nothing on this page is read",
        );
        let corrected = Exception::must("REG-003", Severity::Warn, Scope::Page)
            .with_detail("corrected and verified against the blank form");
        let region = Exception::must("REG-003", Severity::Error, Scope::Region);
        assert!(refusal_in(std::slice::from_ref(&refused)).is_some());
        // a verified correction (WARN) and an unreadable region are not page refusals
        assert!(refusal_in(&[corrected, region]).is_none());
        let line = alignment_notice("3fa2·1");
        assert!(line.starts_with("REG-003 3fa2·1 · not read"), "{line}");
        // R10: what to do, never a guess from this page
        assert!(
            line.contains("rescan the sheet flat") && line.contains("never key"),
            "{line}"
        );
    }

    #[test]
    fn flag_stability_is_read_from_the_job_report() {
        use omr_analytics::{FlagStability, ItemFlag, Rule, StabilityMethod, StabilityReport};
        let flag = |rule: Rule| ItemFlag {
            item: 3,
            master: 3,
            label: "A-Q04".into(),
            rule,
            option: Some(1),
            message: "Probable mis-key: B".into(),
            confidence: omr_analytics::Confidence::Normal,
            stability: None,
        };
        let miskey = flag(Rule::ProbableMiskey);
        assert_eq!(flag_stability(&miskey, None), b::FlagStability::Computing);
        let report = |low: bool, share: f64| StabilityReport {
            method: StabilityMethod::Bootstrap,
            n: 92,
            resamples: 1000,
            run_id: "run".into(),
            seed: String::new(),
            flags: vec![FlagStability {
                item: 3,
                master: 3,
                label: "A-Q04".into(),
                rule: Rule::ProbableMiskey,
                option: Some(1),
                share: Some(share),
                share_mc: None,
                ci_ends: Some((true, !low)),
                low_confidence: low,
            }],
            item_ci: Vec::new(),
        };
        assert!(matches!(
            flag_stability(&miskey, Some(&report(false, 0.97))),
            b::FlagStability::High { share: Some(s), .. } if (s - 0.97).abs() < 1e-12
        ));
        match flag_stability(&miskey, Some(&report(true, 0.31))) {
            b::FlagStability::Low { share, note } => {
                assert_eq!(share, Some(0.31));
                assert!(
                    note.contains("31 %") && note.contains("upper end"),
                    "{note}"
                );
            }
            other => panic!("{other:?}"),
        }
        // a flag the job does not assess (rank > 4) is never "computing"
        let easy = flag(Rule::LittleInformation);
        assert!(easy.rank() > 4);
        assert_eq!(flag_stability(&easy, None), b::FlagStability::NotAssessed);
    }

    #[test]
    fn key_entries_compare_by_their_letters() {
        assert!(same_entry("A|C", "C,A"));
        assert!(same_entry("B (inferred)", "B"));
        assert!(!same_entry("B", "D"));
        assert!(same_entry("*", "*"));
    }

    #[test]
    fn mapping_notes_compress_runs() {
        let mut d = ExamDefinition::new(
            "T",
            TemplateChoice::Legacy("iau-ccsit-v1".into()),
            12,
            5,
            vec!["A".into()],
            omr_model::CancelMode::EraseOnly,
        );
        d.outcomes = Some(omr_app::OutcomeSetup {
            clos: Vec::new(),
            mapping: [3u16, 4, 5, 12]
                .iter()
                .map(|&q| omr_app::CloMapping {
                    question: q,
                    clo: "CLO1".into(),
                    weight: if q == 12 { "0.5".into() } else { "1".into() },
                    note: String::new(),
                    form: String::new(),
                    imported: String::new(),
                    approved_by: String::new(),
                })
                .collect(),
            split: false,
            policy: omr_app::PolicyChoice::Default,
        });
        assert_eq!(mapping_note(&d, "CLO1"), "Q3–5, Q12 · 4 items · Σw 3.5");
        assert_eq!(mapping_note(&d, "CLO2"), "no items mapped");
    }
}
