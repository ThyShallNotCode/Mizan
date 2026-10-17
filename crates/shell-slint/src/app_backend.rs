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
        Some(dir) => ExamProject::grade(&req.definition, &req.inputs, &req.options, &progress)
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

/// An opened project and the derived view-models cached until its next change.
struct Opened {
    p: ExamProject,
    /// `Some(reason)` when the store could not be opened for writing.
    read_only: Option<String>,
    dir: PathBuf,
    items: RefCell<BTreeMap<u64, Option<b::ReviewItem>>>,
    results: RefCell<Option<Option<b::Results>>>,
    outcomes: RefCell<Option<Option<b::Outcomes>>>,
    /// Page thumbnails by page index (pages never change after grading).
    thumbs: RefCell<BTreeMap<usize, Option<b::CropImage>>>,
}

impl Opened {
    fn new(p: ExamProject, dir: PathBuf, read_only: Option<String>) -> Self {
        Self {
            p,
            read_only,
            dir,
            items: RefCell::new(BTreeMap::new()),
            results: RefCell::new(None),
            outcomes: RefCell::new(None),
            thumbs: RefCell::new(BTreeMap::new()),
        }
    }

    /// Drops every cached view-model (after a command changed the project).
    fn changed(&self) {
        self.items.borrow_mut().clear();
        *self.results.borrow_mut() = None;
        *self.outcomes.borrow_mut() = None;
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
        me.opened
            .borrow_mut()
            .insert(id.clone(), Opened::new(p, dir, read_only));
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
        i.roster = path(&form.roster);
        i.clo = path(&form.clo);
        i.title = text(&form.title);
        i.course = text(&form.course);
        let (definition, warnings) = omr_app::define::define(&i).map_err(app_err)?;
        // the inputs exist and hold scans before anything is created
        let files = omr_app::ingest::collect_paths(&inputs).map_err(app_err)?;
        let dir = omr_app::define::new_exam_dir(&workspace, &definition);
        let req = GradeRequest {
            definition: definition.clone(),
            inputs,
            options: GradeOptions {
                threads: 0,
                project_dir: Some(dir.join("project.omrproj")),
                snapshot_utc: None,
                operator: self.operator.clone(),
                max_pages: None,
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
        self.default = Some(exam.clone());
        let key = if i.infer_key {
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
                        .insert(exam.clone(), Opened::new(p, dir, ro));
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
                        .insert(exam.clone(), Opened::new(p, dir, ro));
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
        AppDecision::Acknowledge => b::Decision::Acknowledge,
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
                // the core has no approval workflow: a mapping in the definition is the
                // mapping in force (attainment uses it), never shown as approved
                status: if r.cells.iter().any(|c| c.0.is_some()) {
                    b::MappingStatus::InUse
                } else {
                    b::MappingStatus::Draft
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

    /// A page thumbnail (decoded from the stored original once, then cached).
    fn thumbnail(&self, page: usize) -> Option<b::CropImage> {
        if let Some(hit) = self.thumbs.borrow().get(&page) {
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
        self.thumbs.borrow_mut().insert(page, made.clone());
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
                _ => sheets.review += 1,
            }
        }
        // the highest serials first ("latest"), then pages outside any booklet
        let mut pages: Vec<usize> = (0..s.pages.len())
            .filter(|&p| !matches!(s.pages[p].status, PageStatus::Duplicate | PageStatus::Blank))
            .collect();
        pages.sort_by_key(|&p| {
            std::cmp::Reverse((s.pages[p].booklet.map(|bk| s.booklets[bk].serial), p))
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
        let handled = [
            ("Duplicates ignored (CAP-011)", duplicates),
            ("Blank backs dropped (CAP-012)", blank),
            ("Read turned upright (REG-002)", turned),
        ];
        let shown = handled.into_iter().filter(|(_, n)| *n > 0);
        for (at, (label, n)) in (3..).zip(shown) {
            reconciliation.insert(
                at,
                b::ReconLine {
                    label: label.into(),
                    value: n.to_string(),
                    tone: b::Tone::Neutral,
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
        let notice = v.notice.as_ref().map(|n| b::NoticeEvidence {
            headline: n.headline.clone(),
            lines: n.lines.clone(),
            consequence: n.consequence.clone(),
        });
        let reasons: Vec<String> = if it.kind == ItemKind::Notice {
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
                                .map(|f| b::ItemFlag {
                                    message: f.message.clone(),
                                    code: f.rule.code().map(str::to_string),
                                    tone: match f.rank() {
                                        1 => b::Tone::Error,
                                        2..=4 => b::Tone::Warning,
                                        _ => b::Tone::Info,
                                    },
                                    stable: f.confidence
                                        == omr_analytics::miskey::Confidence::Normal
                                        && f.stability.is_none_or(|x| x >= 0.5),
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
fn short_path(p: &Path) -> String {
    let mut text = p.display().to_string();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = p.strip_prefix(&home)
    {
        text = format!("~/{}", rest.display());
    }
    if text.chars().count() <= 48 {
        return text;
    }
    let parts: Vec<&str> = text.split('/').filter(|s| !s.is_empty()).collect();
    let mut tail = String::new();
    for part in parts.iter().rev() {
        let next = if tail.is_empty() {
            (*part).to_string()
        } else {
            format!("{part}/{tail}")
        };
        if next.chars().count() > 44 {
            break;
        }
        tail = next;
    }
    format!("…/{tail}")
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
            // read by omr-app (rule R4), as the key of a new exam is
            let (forms, points) =
                omr_app::define::read_key_path(path, &o.p.session.definition).map_err(app_err)?;
            o.p.replace_key(forms, points, None).map_err(app_err)?;
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
        self.drain_job()
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
                })
                .collect(),
            split: false,
            policy: omr_app::PolicyChoice::Default,
        });
        assert_eq!(mapping_note(&d, "CLO1"), "Q3–5, Q12 · 4 items · Σw 3.5");
        assert_eq!(mapping_note(&d, "CLO2"), "no items mapped");
    }
}
