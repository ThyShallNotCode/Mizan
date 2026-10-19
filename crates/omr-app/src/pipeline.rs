//! The grading pipeline and its scheduler (§4.5, §8.1, §21.2). The scheduler owns the rayon
//! pool (R4): pages are decoded and measured in parallel (one task per page), then the batch
//! stage runs per batch slice (capture class × device resolution) in page-ID order, identity
//! and scoring follow, and everything lands in the project store. Outputs do not depend on the
//! thread count or the input order (A5): pages are ordered by content (source SHA-384, page
//! index) and batch statistics are order-independent.

use crate::AppError;
use crate::engine::{
    MAX_TRUSTED_DPI, MIN_TRUSTED_DPI, Measured, PageRead, Recognizer, WORKING_DPI,
};
use crate::exam::{ExamDefinition, KeySource, ParsedKey, parse_key_entry};
use crate::infer;
use crate::ingest::{self, OpenSource, SourceFile, SourceKind};
use crate::model::{
    Booklet, EffectiveKey, FormSource, IdentityState, KeyCell, PageState, PageStatus,
    QuestionState, ResponseStatus, SESSION_SCHEMA, Session, SourceInfo, Timings,
};
use crate::review::{ReviewState, build_queue};
use crate::score::Scoring;
use crate::store::StoreWriter;
use omr_identity::{
    Assigned, BookletEvidence, BookletIdentity, IdentityPolicy, IdentityStatus, ImportOptions,
    PriorContext, RosterRecord, RosterSnapshot,
};
use omr_model::exceptions::Target;
use omr_model::{Exception, Response, Scope, Severity};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The operator's cancel of a grading run or a survey (G-3): a shared flag the shell sets from
/// another thread. The run checks it before the project package is created, before each
/// page's decoding and between stages, never once the store stage has begun: a cancelled new
/// exam stores nothing ([`AppError::Cancelled`]; its package is removed). A cancelled run of a
/// graded exam (scans added, a roster given) leaves its results as they were but keeps what its
/// spool stored: a new scan batch, left open, with the originals and page rows of the scans
/// added; adding the same files again adds each page once. Clones share the flag; a token
/// nobody cancels (the default) never stops a run.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// Asks every run holding this token (or a clone) to stop at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// True once [`Self::cancel`] was called on this token or a clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// [`AppError::Cancelled`] once cancelled.
    pub(crate) fn check(&self) -> Result<(), AppError> {
        if self.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Two requests are the same when they share the token (a fresh token is another request).
impl PartialEq for Cancel {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// How to run a grading job.
#[derive(Clone, Debug)]
pub struct GradeOptions {
    /// Worker threads (0 = available parallelism).
    pub threads: usize,
    /// Project package to create (`None`: in memory only, e.g. benchmarks).
    pub project_dir: Option<PathBuf>,
    /// Report snapshot time (RFC 3339 UTC); `None` = now.
    pub snapshot_utc: Option<String>,
    /// Operator principal recorded in events.
    pub operator: String,
    /// Grade only the first `n` pages in input order (time-to-first-result runs).
    pub max_pages: Option<usize>,
    /// The operator's cancel (default: never cancelled).
    pub cancel: Cancel,
}

impl Default for GradeOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            project_dir: None,
            snapshot_utc: None,
            operator: "principal:operator".into(),
            max_pages: None,
            cancel: Cancel::default(),
        }
    }
}

/// A provisional page result (scanner-following display, §8.1 page stage).
#[derive(Clone, Debug, PartialEq)]
pub struct ProvisionalPage {
    /// Page ID.
    pub page_id: String,
    /// Questions read.
    pub questions: usize,
    /// Auto-accepted.
    pub auto_accepted: usize,
    /// Worst page code, if any.
    pub worst_code: Option<(String, Severity)>,
}

/// Batch progress events (coalesced by the UI, §4.5).
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Inputs enumerated.
    Started {
        /// Pages to process.
        pages: usize,
        /// Worker threads.
        workers: usize,
    },
    /// One page through the page stage (completion order).
    Page {
        /// Pages done.
        done: usize,
        /// Pages total.
        total: usize,
        /// Milliseconds since start.
        elapsed_ms: f64,
        /// Provisional result.
        page: ProvisionalPage,
    },
    /// A pipeline stage started.
    Stage(&'static str),
    /// Finished.
    Finished {
        /// Total milliseconds.
        total_ms: f64,
    },
}

/// Current UTC time as RFC 3339 (seconds).
pub fn now_utc() -> String {
    omr_store::clock::rfc3339_utc(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64),
    )
}

/// Checks a report snapshot time: RFC 3339 UTC, `YYYY-MM-DDTHH:MM:SS[.fff]Z` (§17.1). Checked
/// before any work, so a bad value never leaves a graded project without its exports.
pub fn check_snapshot_utc(s: &str) -> Result<(), AppError> {
    let bad = || {
        AppError::Definition(format!(
            "snapshot time {s:?} is not RFC 3339 UTC (YYYY-MM-DDTHH:MM:SS[.fff]Z)"
        ))
    };
    let b = s.as_bytes();
    if b.len() < 20 || !matches!(b[b.len() - 1], b'Z' | b'z') {
        return Err(bad());
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        s.get(r)
            .filter(|t| t.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|t| t.parse().ok())
    };
    let seps = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if !seps.iter().all(|&(i, c)| b[i] == c) || !matches!(b[10], b'T' | b't') {
        return Err(bad());
    }
    let (Some(_y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return Err(bad());
    };
    let frac = &s[19..s.len() - 1];
    let frac_ok = frac.is_empty()
        || (frac.len() >= 2
            && frac.starts_with('.')
            && frac[1..].bytes().all(|c| c.is_ascii_digit()));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 || !frac_ok
    {
        return Err(bad());
    }
    Ok(())
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// A page task: which source and page, plus ingest outcome.
#[derive(Clone)]
struct PageTask {
    page_id: String,
    source: usize,
    index: u32,
    duplicate_of: Option<usize>,
    input_order: usize,
}

/// Output of the page stage for one page.
struct Staged {
    measured: Option<Measured>,
    refusal: Option<Exception>,
    capture: Option<omr_imaging::CaptureKind>,
    source_dpi: Option<u32>,
    working: Option<(u32, u32)>,
    /// A blank back or bleed-through only (CAP-012 in `refusal`).
    blank: bool,
    /// Page notes added to the decided page (CAP-002 WARN/INFO).
    notes: Vec<Exception>,
}

impl Staged {
    /// Nothing read (a duplicate's page, or a page a cancelled run skipped).
    fn empty() -> Self {
        Self {
            measured: None,
            refusal: None,
            capture: None,
            source_dpi: None,
            working: None,
            blank: false,
            notes: Vec::new(),
        }
    }
}

/// Dark-pixel ceiling of a blank page (§8.9 CAP-012: < 0.2 % dark pixels and no tags).
pub const BLANK_MAX_DARK_PPM: u32 = 2_000;

/// Effective capture resolutions of CAP-002 (§8.9): below the first a page is not graded
/// (ERROR), up to the second it is read with a warning, above the third it was read from a
/// downsampled working copy (INFO).
pub const CAP002_DPI: [u32; 3] = [100, 150, 400];

/// CAP-002 for an effective capture resolution of `dpi`, if one applies.
pub fn cap002(page_id: &str, dpi: u32) -> Option<Exception> {
    let [reject, warn, info] = CAP002_DPI;
    let (sev, detail) = if dpi < reject {
        (
            Severity::Error,
            format!(
                "effective resolution {dpi} dpi is below {reject} dpi: the page is not graded; \
                 rescan it at 200–300 dpi"
            ),
        )
    } else if dpi <= warn {
        (
            Severity::Warn,
            format!(
                "effective resolution {dpi} dpi ({reject}–{warn} dpi): read, but light marks are \
                 less reliable; scan at 200–300 dpi"
            ),
        )
    } else if dpi > info {
        (
            Severity::Info,
            format!(
                "captured at {dpi} dpi: read from a working copy downsampled to {WORKING_DPI} dpi"
            ),
        )
    } else {
        return None;
    };
    Some(
        Exception::must("CAP-002", sev, Scope::Page)
            .with_target(Target {
                page: Some(page_id.to_string()),
                ..Target::default()
            })
            .with_detail(detail),
    )
}

/// CAP-012 (INFO, Page): a blank page or bleed-through only, dropped (§7.5).
pub fn cap012(page_id: &str, dark_ppm: u32) -> Exception {
    Exception::must("CAP-012", Severity::Info, Scope::Page)
        .with_target(Target {
            page: Some(page_id.to_string()),
            ..Target::default()
        })
        .with_detail(format!(
            "blank page or bleed-through only ({:.2} % dark pixels, no form found); dropped, \
             nothing to grade",
            f64::from(dark_ppm) / 10_000.0
        ))
}

/// Builds the rayon pool of a job (the only place pools are created, R4).
pub fn pool(threads: usize) -> Result<rayon::ThreadPool, AppError> {
    let n = if threads == 0 {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        threads
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .thread_name(|i| format!("omr-page-{i}"))
        .panic_handler(|_| {})
        .build()
        .map_err(|e| AppError::Internal(format!("thread pool: {e}")))
}

/// Everything the pipeline produced.
pub struct Graded {
    /// The exam state.
    pub session: Session,
    /// Scoring state (`None` without a key).
    pub scoring: Option<Scoring>,
    /// The open store (when a project directory was given).
    pub store: Option<StoreWriter>,
}

/// Grades `inputs` (files and folders) for an exam: the whole pipeline.
pub fn grade(
    def: &ExamDefinition,
    inputs: &[PathBuf],
    opts: &GradeOptions,
    progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Graded, AppError> {
    let paths = ingest::collect_paths(inputs)?;
    let t0 = Instant::now();
    let pool = pool(opts.threads)?;
    let sources: Vec<Result<SourceFile, (PathBuf, String)>> = pool.install(|| {
        paths
            .par_iter()
            .map(|p| SourceFile::read(p).map_err(|e| (p.clone(), e)))
            .collect()
    });
    grade_sources(def, sources, opts, &pool, t0, progress)
}

/// [`grade`] with key-sheet files (`omr grade --key-sheet`, §10.1): every sheet in
/// `key_sheets` is a key sheet (read by the same recognizer, routed to the key store, held
/// for the instructor's confirmation); `inputs` may hold key sheets too, found by the rule.
pub fn grade_with_key_sheets(
    def: &ExamDefinition,
    inputs: &[PathBuf],
    key_sheets: &[PathBuf],
    opts: &GradeOptions,
    progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Graded, AppError> {
    let ks = if key_sheets.is_empty() {
        Vec::new()
    } else {
        ingest::collect_paths(key_sheets)?
    };
    let mut paths = if inputs.is_empty() {
        Vec::new()
    } else {
        ingest::collect_paths(inputs)?
    };
    // A file named as a key sheet is a key sheet, also when a folder of scans holds it.
    paths.retain(|p| !ks.contains(p));
    paths.extend(ks.iter().cloned());
    let t0 = Instant::now();
    let pool = pool(opts.threads)?;
    let sources: Vec<Result<SourceFile, (PathBuf, String)>> = pool.install(|| {
        paths
            .par_iter()
            .map(|p| SourceFile::read(p).map_err(|e| (p.clone(), e)))
            .collect()
    });
    let mut d = def.clone();
    for (p, s) in paths.iter().zip(&sources) {
        if !ks.contains(p) {
            continue;
        }
        match s {
            Ok(f) => {
                if !d.key_sheets.declared.contains(&f.sha384) {
                    d.key_sheets.declared.push(f.sha384.clone());
                }
            }
            Err((_, e)) => {
                return Err(AppError::Input(format!(
                    "key sheet {}: {e}; check the path or rescan the key sheet",
                    p.display()
                )));
            }
        }
    }
    d.key_sheets.declared.sort();
    grade_sources(&d, sources, opts, &pool, t0, progress)
}

/// Grades sources already in memory (tests, benchmarks).
pub fn grade_sources(
    def: &ExamDefinition,
    sources: Vec<Result<SourceFile, (PathBuf, String)>>,
    opts: &GradeOptions,
    pool: &rayon::ThreadPool,
    t0: Instant,
    progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Graded, AppError> {
    grade_into(def, sources, opts, pool, t0, progress, None)
}

/// A batch appended to a graded exam: its open store and the exam state before the batch.
pub struct Append<'a> {
    /// The project store (a new scan batch and new runs are written into it).
    pub store: StoreWriter,
    /// The exam before the batch (key, resolutions and serials carry over from it).
    pub prior: &'a Session,
}

/// [`grade_sources`], or with `append` the re-run of an exam over its stored originals and a
/// new batch (`sources` holds both): new batch and runs in the same store, the key in force
/// and the human resolutions carried over ([`crate::append`]).
pub fn grade_into(
    def: &ExamDefinition,
    sources: Vec<Result<SourceFile, (PathBuf, String)>>,
    opts: &GradeOptions,
    pool: &rayon::ThreadPool,
    t0: Instant,
    progress: &(dyn Fn(&Progress) + Sync),
    append: Option<Append<'_>>,
) -> Result<Graded, AppError> {
    def.validate()?;
    let snapshot_utc = opts.snapshot_utc.clone().unwrap_or_else(now_utc);
    check_snapshot_utc(&snapshot_utc)?;
    let recognizer = Recognizer::new(def)?;
    let mut timings = Timings {
        threads: pool.current_num_threads(),
        ..Timings::default()
    };

    // ---- Ingest: canonical order, duplicates, unreadable files.
    let mut files: Vec<(usize, SourceFile)> = Vec::new();
    let mut broken: Vec<(usize, PathBuf, String)> = Vec::new();
    for (k, s) in sources.into_iter().enumerate() {
        match s {
            // Every zero-byte file is a lost scan of its own (CAP-001), never a byte-identical
            // duplicate of another empty file (all of them share one hash).
            Ok(f) if f.bytes.is_empty() => {
                broken.push((k, f.path.clone(), "empty (zero-byte) file".into()));
            }
            Ok(f) => files.push((k, f)),
            Err((p, e)) => broken.push((k, p, e)),
        }
    }
    // Byte-identical files: the original is the first by name and path, never by input
    // position, so the same set of files yields the same outputs in any order (A5). In an
    // appended batch the copy already ingested stays the original (§7.4: a duplicate links to
    // the existing object).
    let ingested: std::collections::BTreeSet<(&str, &str)> = append
        .as_ref()
        .map(|a| {
            a.prior
                .sources
                .iter()
                .map(|s| (s.sha384.as_str(), s.name.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let fresh = |f: &SourceFile| !ingested.contains(&(f.sha384.as_str(), f.name.as_str()));
    files.sort_by(|a, b| {
        a.1.sha384
            .cmp(&b.1.sha384)
            .then_with(|| fresh(&a.1).cmp(&fresh(&b.1)))
            .then_with(|| a.1.name.cmp(&b.1.name))
            .then_with(|| a.1.path.cmp(&b.1.path))
            .then(a.0.cmp(&b.0))
    });
    // Unreadable files follow, by path (not by input position: A5).
    broken.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut source_infos: Vec<SourceInfo> = Vec::new();
    let mut source_files: Vec<Option<SourceFile>> = Vec::new();
    let mut first_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut dup_of: Vec<Option<usize>> = Vec::new();
    let mut input_order_of: Vec<usize> = Vec::new();
    let mut unreadable: Vec<Option<String>> = Vec::new();
    for (k, f) in files {
        let dup = first_of.get(&f.sha384).copied();
        if dup.is_none() {
            first_of.insert(f.sha384.clone(), source_infos.len());
        }
        source_infos.push(SourceInfo {
            name: f.name.clone(),
            sha384: f.sha384.clone(),
            blake3: None,
            kind: f.kind,
            pages: 0,
        });
        dup_of.push(dup);
        input_order_of.push(k);
        source_files.push(Some(f));
        unreadable.push(None);
    }
    for (k, p, e) in broken {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        unreadable.push(Some(format!("{name}: {e}")));
        let sha = omr_store::jcs::sha384_hex(format!("unreadable:{}", p.display()).as_bytes());
        source_infos.push(SourceInfo {
            name: format!("{name} (unreadable: {e})"),
            sha384: sha,
            blake3: None,
            kind: SourceKind::Image,
            pages: 1,
        });
        dup_of.push(None);
        input_order_of.push(k);
        source_files.push(None);
    }
    // Open sources (PDF parse) in parallel.
    let opened: Vec<(OpenSource, u32)> = pool.install(|| {
        source_files
            .par_iter()
            .zip(dup_of.par_iter())
            .zip(unreadable.par_iter())
            .map(|((f, dup), why)| match (f, dup) {
                (Some(_), Some(_)) => (OpenSource::Broken("duplicate".into()), 0),
                (Some(f), None) => ingest::open_source(f),
                (None, _) => (
                    OpenSource::Broken(
                        why.clone()
                            .unwrap_or_else(|| "file could not be read".into()),
                    ),
                    1,
                ),
            })
            .collect()
    });
    let mut tasks: Vec<PageTask> = Vec::new();
    for (s, (_, n)) in opened.iter().enumerate() {
        let n = if dup_of[s].is_some() { 1 } else { *n };
        source_infos[s].pages = n;
        for i in 0..n {
            tasks.push(PageTask {
                page_id: if dup_of[s].is_some() {
                    format!("{}-dup{s}", ingest::page_id(&source_infos[s].sha384, i))
                } else {
                    ingest::page_id(&source_infos[s].sha384, i)
                },
                source: s,
                index: i,
                duplicate_of: dup_of[s],
                input_order: input_order_of[s] * 100_000 + i as usize,
            });
        }
    }

    if let Some(n) = opts.max_pages {
        let mut order: Vec<usize> = (0..tasks.len()).collect();
        order.sort_by_key(|&i| tasks[i].input_order);
        let keep: std::collections::BTreeSet<usize> = order.into_iter().take(n).collect();
        let mut k = 0;
        tasks.retain(|_| {
            let r = keep.contains(&k);
            k += 1;
            r
        });
        for (s, info) in source_infos.iter_mut().enumerate() {
            info.pages = tasks.iter().filter(|t| t.source == s).count() as u32;
        }
    }

    // ---- Spool first (§7.4): originals durable in the store before any computation. A run
    // cancelled by now creates no package.
    opts.cancel.check()?;
    let (mut store, prior_run) = match append {
        Some(a) => (Some(a.store), Some(a.prior)),
        None => match &opts.project_dir {
            Some(dir) => (
                Some(StoreWriter::create(
                    dir,
                    def,
                    &snapshot_utc,
                    &opts.operator,
                )?),
                None,
            ),
            None => (None, None),
        },
    };
    if let Some(st) = store.as_mut() {
        // An appended batch: originals already in the store keep their address and pages.
        let stored: BTreeMap<String, String> = prior_run
            .iter()
            .flat_map(|p| &p.sources)
            .filter_map(|s| s.blake3.clone().map(|b| (s.sha384.clone(), b)))
            .collect();
        let known: std::collections::BTreeSet<String> = prior_run
            .iter()
            .flat_map(|p| &p.pages)
            .map(|pg| pg.page_id.clone())
            .collect();
        if prior_run.is_some() {
            st.begin_append()?;
        } else {
            st.write_setup(def, recognizer.manifest())?;
        }
        let to_stage: Vec<Option<SourceFile>> = source_files
            .iter()
            .map(|f| f.clone().filter(|f| !stored.contains_key(&f.sha384)))
            .collect();
        let blake = pool.install(|| st.stage_sources(&to_stage, &dup_of))?;
        for (s, b) in blake.into_iter().enumerate() {
            source_infos[s].blake3 = b.or_else(|| stored.get(&source_infos[s].sha384).cloned());
        }
        st.spool_pages(
            &tasks
                .iter()
                .map(|t| (t.page_id.clone(), t.source, t.index, t.duplicate_of))
                .collect::<Vec<_>>(),
            &source_infos,
            &known,
        )?;
    }
    timings.ingest_ms = ms(t0);

    // ---- Page stage: decode + working resolution + measurement, one task per page.
    progress(&Progress::Started {
        pages: tasks.len(),
        workers: pool.current_num_threads(),
    });
    let done = AtomicUsize::new(0);
    let first = AtomicU64::new(u64::MAX);
    let total = tasks.len();
    let prior = recognizer.prior_baseline();
    let t_page = Instant::now();
    let staged: Vec<Staged> = pool.install(|| {
        tasks
            .par_iter()
            .map(|t| {
                // A cancelled run decodes no further page (the run stops after this stage).
                if opts.cancel.is_cancelled() {
                    return Staged::empty();
                }
                let out = if t.duplicate_of.is_some() {
                    Staged::empty()
                } else {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        stage_page(
                            &recognizer,
                            source_files[t.source].as_ref(),
                            &opened[t.source].0,
                            t,
                        )
                    }));
                    r.unwrap_or_else(|_| Staged {
                        measured: None,
                        refusal: Some(ingest::cap001(&t.page_id, "decoder or engine panic")),
                        capture: None,
                        source_dpi: None,
                        working: None,
                        blank: false,
                        notes: Vec::new(),
                    })
                };
                // Provisional result for the live view (single-page prior).
                let provisional = out.measured.as_ref().and_then(|m| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        recognizer.decide(m, &prior, &t.page_id)
                    }))
                    .ok()
                });
                let prov = match provisional {
                    Some(r) => ProvisionalPage {
                        page_id: t.page_id.clone(),
                        questions: r.questions.len(),
                        auto_accepted: r.questions.iter().filter(|q| q.auto_accepted).count(),
                        worst_code: r
                            .exceptions
                            .iter()
                            .max_by_key(|e| e.severity)
                            .map(|e| (e.code.clone(), e.severity)),
                    },
                    None => ProvisionalPage {
                        page_id: t.page_id.clone(),
                        questions: 0,
                        auto_accepted: 0,
                        worst_code: out.refusal.as_ref().map(|e| (e.code.clone(), e.severity)),
                    },
                };
                let elapsed = ms(t0);
                first.fetch_min((elapsed * 1000.0) as u64, Ordering::Relaxed);
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress(&Progress::Page {
                    done: d,
                    total,
                    elapsed_ms: elapsed,
                    page: prov,
                });
                out
            })
            .collect()
    });
    timings.page_stage_ms = ms(t_page);
    timings.first_result_ms = match first.load(Ordering::Relaxed) {
        u64::MAX => 0.0,
        v => v as f64 / 1000.0,
    };
    timings.pages = tasks.len();
    opts.cancel.check()?;

    // ---- Batch stage: baseline per slice (capture class × device dpi), then decisions.
    // Key sheets (§10.1) are read with the slice's baseline but never shape it: the students
    // read exactly as without them. Named key sheets are known now; key sheets found in the
    // stack by the rule are found after a first decision pass, and their slices are decided
    // again without them.
    progress(&Progress::Stage("batch"));
    let t_batch = Instant::now();
    let declared: std::collections::BTreeSet<&str> =
        def.key_sheets.declared.iter().map(String::as_str).collect();
    let is_declared: Vec<bool> = tasks
        .iter()
        .map(|t| {
            t.duplicate_of.is_none() && declared.contains(source_infos[t.source].sha384.as_str())
        })
        .collect();
    let mut key_pages: std::collections::BTreeSet<usize> =
        (0..tasks.len()).filter(|&i| is_declared[i]).collect();
    let mut slices: BTreeMap<(bool, Option<u32>), Vec<usize>> = BTreeMap::new();
    for (i, s) in staged.iter().enumerate() {
        if let Some(m) = &s.measured {
            slices
                .entry((m.bitonal(), s.source_dpi))
                .or_default()
                .push(i);
        }
    }
    let slice_baseline = |idx: &[usize], keys: &std::collections::BTreeSet<usize>| {
        let students: Vec<&Measured> = idx
            .iter()
            .filter(|i| !keys.contains(i))
            .filter_map(|&i| staged[i].measured.as_ref())
            .collect();
        if students.is_empty() {
            let all: Vec<&Measured> = idx
                .iter()
                .filter_map(|&i| staged[i].measured.as_ref())
                .collect();
            recognizer.baseline(&all)
        } else {
            recognizer.baseline(&students)
        }
    };
    let mut baselines = BTreeMap::new();
    for (key, idx) in &slices {
        baselines.insert(*key, slice_baseline(idx, &key_pages));
    }
    // A panic while deciding one page quarantines that page (CAP-001), never the batch.
    let decide_pages = |only: Option<&std::collections::BTreeSet<usize>>,
                        baselines: &BTreeMap<(bool, Option<u32>), crate::engine::BatchBaseline>|
     -> Vec<Option<Result<Option<PageRead>, ()>>> {
        pool.install(|| {
            staged
                .par_iter()
                .zip(tasks.par_iter())
                .enumerate()
                .map(|(i, (s, t))| {
                    if only.is_some_and(|o| !o.contains(&i)) {
                        return None;
                    }
                    let Some(m) = s.measured.as_ref() else {
                        return Some(Ok(None));
                    };
                    let b = &baselines[&(m.bitonal(), s.source_dpi)];
                    Some(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            recognizer.decide(m, b, &t.page_id)
                        }))
                        .map(Some)
                        .map_err(|_| ()),
                    )
                })
                .collect()
        })
    };
    let mut reads: Vec<Result<Option<PageRead>, ()>> = decide_pages(None, &baselines)
        .into_iter()
        .map(|r| r.unwrap_or(Ok(None)))
        .collect();
    // Key sheets in the stack (§10.1 rule): a student-number grid bubbled with a marker no
    // student can have, and a declared version.
    let mut stack_notes = Vec::new();
    if def.key_sheets.detect_in_stack {
        let mut by_digits: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        let mut found = std::collections::BTreeSet::new();
        for (i, r) in reads.iter().enumerate() {
            let Ok(Some(r)) = r else {
                continue;
            };
            if is_declared[i] || tasks[i].duplicate_of.is_some() {
                continue;
            }
            let digits = r
                .student_id
                .as_ref()
                .map_or(0, |g| g.text().chars().count());
            let markers = by_digits.entry(digits).or_insert_with(|| {
                let (m, notes) = crate::keysheet::stack_markers(def, digits);
                for n in notes {
                    if !stack_notes.contains(&n) {
                        stack_notes.push(n);
                    }
                }
                m
            });
            if crate::keysheet::is_stack_key_sheet(r, markers) {
                found.insert(i);
            }
        }
        if !found.is_empty() {
            let affected: Vec<(bool, Option<u32>)> = slices
                .iter()
                .filter(|(_, idx)| idx.iter().any(|i| found.contains(i)))
                .map(|(k, _)| *k)
                .collect();
            key_pages.extend(found);
            let mut redo = std::collections::BTreeSet::new();
            for k in affected {
                let idx = &slices[&k];
                baselines.insert(k, slice_baseline(idx, &key_pages));
                redo.extend(idx.iter().copied());
            }
            for (i, r) in decide_pages(Some(&redo), &baselines)
                .into_iter()
                .enumerate()
            {
                if let Some(r) = r {
                    reads[i] = r;
                }
            }
        }
    }
    // Key sheets that mark rows beyond the exam's questions (§10.1): the key pages are read
    // once more at the sheet's printed count, and the marked rows past the exam's count are
    // named in the key summary (never part of the key), as a key file's are.
    let printed = recognizer.printed_questions();
    let mut beyond: Vec<Vec<u16>> = vec![Vec::new(); tasks.len()];
    if printed > def.questions {
        for &i in &key_pages {
            let (Some(m), Ok(Some(_))) = (staged[i].measured.as_ref(), &reads[i]) else {
                continue;
            };
            let b = &baselines[&(m.bitonal(), staged[i].source_dpi)];
            let full = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                recognizer.decide_rows(m, b, &tasks[i].page_id, printed)
            }));
            if let Ok(full) = full {
                beyond[i] = full
                    .questions
                    .iter()
                    .filter(|q| q.question >= def.questions)
                    .filter(|q| {
                        matches!(
                            q.response,
                            Response::Single(_) | Response::Multi(_) | Response::Set(_)
                        )
                    })
                    .map(|q| q.question + 1)
                    .collect();
            }
        }
    }
    let mut batch_exceptions: Vec<Exception> = Vec::new();
    for b in baselines.values() {
        for e in b.exceptions() {
            push_batch(&mut batch_exceptions, e);
        }
    }
    let mut pages: Vec<PageState> = Vec::with_capacity(tasks.len());
    for ((t, mut s), r) in tasks.iter().zip(staged).zip(reads) {
        let mut r = match r {
            Ok(r) => r,
            Err(()) => {
                s.refusal
                    .get_or_insert_with(|| ingest::cap001(&t.page_id, "decision engine panic"));
                None
            }
        };
        if let Some(r) = r.as_mut() {
            r.exceptions.append(&mut s.notes);
        }
        // Batch-scope exceptions (CAP-003 bitonal) are de-duplicated per batch (§13.2).
        if let Some(r) = r.as_mut() {
            r.exceptions.retain(|e| {
                if e.scope == Scope::Batch {
                    push_batch(&mut batch_exceptions, e);
                    false
                } else {
                    true
                }
            });
        }
        let i = pages.len();
        let status = if t.duplicate_of.is_some() {
            PageStatus::Duplicate
        } else if s.blank {
            PageStatus::Blank
        } else if key_pages.contains(&i) {
            PageStatus::KeySheet
        } else if s.refusal.is_some() {
            PageStatus::Refused
        } else if r.as_ref().is_some_and(|r| r.registered) {
            PageStatus::Read
        } else {
            PageStatus::Unattributed
        };
        // A page that is not attributed to this exam is never dropped silently: when the
        // engine raised nothing blocking (e.g. a verified page code of another template
        // revision), it is a foreign form (IDN-002, BLOCKER) for the Capture lane.
        if status == PageStatus::Unattributed
            && let Some(r) = r.as_mut()
            && !r.exceptions.iter().any(|e| e.severity >= Severity::Error)
        {
            r.exceptions.push(
                Exception::must("IDN-002", Severity::Blocker, Scope::Page)
                    .with_target(Target {
                        page: Some(t.page_id.clone()),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "page not attributed to this exam's sheet ({}); never graded",
                        r.registration
                            .get("route")
                            .and_then(|v| v.as_str())
                            .unwrap_or("not registered")
                    )),
            );
        }
        let refusal = s.refusal.or_else(|| {
            t.duplicate_of.map(|d| {
                Exception::must("CAP-011", Severity::Info, Scope::Page)
                    .with_target(Target {
                        page: Some(t.page_id.clone()),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "byte-identical to {} already ingested",
                        source_infos[d].name
                    ))
            })
        });
        pages.push(PageState {
            page_id: t.page_id.clone(),
            source: t.source,
            index: t.index,
            status,
            capture: s.capture,
            source_dpi: s.source_dpi,
            working: s.working,
            read: r,
            refusal,
            booklet: None,
        });
    }
    timings.batch_stage_ms = ms(t_batch);

    // ---- Key sheets out of the student population (§10.1): their pages go after every other
    // page (each part in canonical order), so the students' pages keep the indices of a run
    // without key sheets; then collation in canonical order.
    let mut is_declared = is_declared;
    if pages.iter().any(|p| p.status == PageStatus::KeySheet) {
        let mut order: Vec<usize> = (0..pages.len()).collect();
        order.sort_by_key(|&i| pages[i].status == PageStatus::KeySheet);
        pages = order.iter().map(|&i| pages[i].clone()).collect();
        tasks = order.iter().map(|&i| tasks[i].clone()).collect();
        is_declared = order.iter().map(|&i| is_declared[i]).collect();
        beyond = order.iter().map(|&i| beyond[i].clone()).collect();
    }
    let mut key_sheets = crate::model::KeySheets {
        sheets: crate::keysheet::collect(def, &pages, &source_infos, &is_declared, &beyond),
        notes: stack_notes,
        ..Default::default()
    };
    crate::keysheet::refresh_pending(&mut key_sheets);
    let mut booklets = collate(def, &recognizer, &mut pages, &tasks);
    // Phone photos of students' sheets (§8.10): the phone slice is not calibrated, so their
    // answers went to review (MRK-001 `uncalibrated_capture`); the batch says why once.
    let photos = booklets
        .iter()
        .flat_map(|b| &b.pages)
        .filter(|&&pi| pages[pi].read.as_ref().is_some_and(|r| r.photo))
        .count();
    if photos > 0 {
        push_batch(
            &mut batch_exceptions,
            &Exception::must("CAP-003", Severity::Warn, Scope::Batch)
                .with_sub_reason(PHONE_UNCALIBRATED)
                .with_detail(format!(
                    "{photos} student page(s) are phone photos: the phone capture is not \
                     calibrated yet (§8.10 step 7), so every answer on them waits for a person; \
                     scan the sheets on a scanner instead"
                )),
        );
    }

    // ---- Identity (§9).
    opts.cancel.check()?;
    progress(&Progress::Stage("identity"));
    let t_id = Instant::now();
    identify(def, &mut booklets, &pages)?;
    timings.identity_ms = ms(t_id);

    // ---- Forms, key, inference, scoring (§10).
    opts.cancel.check()?;
    progress(&Progress::Stage("scoring"));
    let t_sc = Instant::now();
    let mut exam_exceptions = Vec::new();
    let mut key = resolve_key(def, &mut booklets, &mut exam_exceptions, &key_sheets)?;
    if let Some(p) = prior_run {
        crate::append::carry_key(p, &mut key);
        crate::keysheet::carry(
            def,
            p,
            &mut key_sheets,
            &mut key,
            &mut exam_exceptions,
            &source_infos,
            &pages,
        );
    }
    // Never scored from an unconfirmed key-sheet key: the run is held (SCO-003 BLOCKER).
    if key_sheets.pending {
        exam_exceptions.push(crate::keysheet::hold_exception(def, &key_sheets));
    }
    let mut session = Session {
        schema: SESSION_SCHEMA,
        definition: def.clone(),
        snapshot_utc,
        manifest: recognizer.manifest(),
        sources: source_infos,
        pages,
        booklets,
        key,
        batch_exceptions,
        exam_exceptions,
        review: ReviewState::default(),
        timings: Timings::default(),
        release: None,
        key_sheets,
        flags: Default::default(),
    };
    // ---- Review queue (an appended batch carries the earlier run's decisions over).
    let labels = def.option_labels();
    let digits: Vec<String> = (0..10).map(|d| d.to_string()).collect();
    if let Some(p) = prior_run {
        crate::append::mark_conflicts(p, &mut session);
    }
    session.review.items = build_queue(&session, &labels, &digits);
    if let Some(p) = prior_run {
        let (_, ids) = crate::append::carry_review(p, &mut session);
        // Earlier review events stay in the history, sealed (their decisions now belong to
        // this run's items).
        session.review.history = p.review.history.clone();
        for r in &mut session.review.history {
            r.item = ids.get(&r.item).copied().unwrap_or(r.item);
            for c in &mut r.changes {
                c.item = ids.get(&c.item).copied().unwrap_or(c.item);
            }
        }
        session.review.sealed = session.review.history.len();
    }
    let scoring = Scoring::build(&session)?;
    timings.scoring_ms = ms(t_sc);

    // ---- Store: recognition run, identity, exceptions, score run. The last check: once the
    // store stage has begun the run is written whole.
    opts.cancel.check()?;
    progress(&Progress::Stage("store"));
    let t_st = Instant::now();
    if let Some(st) = store.as_mut() {
        st.write_results(&mut session, scoring.as_ref(), prior_run)?;
    }
    timings.store_ms = ms(t_st);
    timings.total_ms = ms(t0);
    session.timings = timings;
    progress(&Progress::Finished {
        total_ms: session.timings.total_ms,
    });
    Ok(Graded {
        session,
        scoring,
        store,
    })
}

fn push_batch(list: &mut Vec<Exception>, e: &Exception) {
    if !list
        .iter()
        .any(|x| x.code == e.code && x.sub_reason == e.sub_reason)
    {
        let mut e = e.clone();
        e.target = Target::default();
        list.push(e);
    }
}

/// Decode, working resolution and page stage for one page.
fn stage_page(
    rec: &Recognizer,
    src: Option<&SourceFile>,
    open: &OpenSource,
    t: &PageTask,
) -> Staged {
    let none = |e: Exception| Staged {
        measured: None,
        refusal: Some(e),
        capture: None,
        source_dpi: None,
        working: None,
        blank: false,
        notes: Vec::new(),
    };
    let Some(src) = src else {
        let why = match open {
            OpenSource::Broken(why) => why.as_str(),
            _ => "file could not be read",
        };
        return none(ingest::cap001(&t.page_id, why));
    };
    let d = match ingest::decode_page(src, open, t.index, &t.page_id) {
        Ok(d) => d,
        Err(e) => return none(e),
    };
    let capture = d.capture;
    let decoded_long = d.image.width().max(d.image.height());
    let d_source_dpi = d.source_dpi;
    let w = rec.to_working(d.image, d.capture, d.known_dpi);
    let working = Some((w.image.width() as u32, w.image.height() as u32));
    let source_dpi = d.source_dpi.or(w.source_dpi);
    let measured = rec.measure(&w, &t.page_id);
    // Blank backs and bleed-through (§7.5, §8.9): < 0.2 % dark pixels and no form. A page
    // with that little ink is dropped (CAP-012) only when the engine finds no sheet on it
    // either: an own form without tags, or with only mirrored tags (the front seen through
    // the paper); a legacy form that does not register. A washed-out scan of a real sheet
    // still registers and is graded (and queued if its marks are faint).
    let no_sheet = match &measured {
        Measured::Legacy(m, _) => !m.registered(),
        Measured::Own(m) => m.tags.tags.is_empty() || m.tags.mirrored,
    };
    // (The ink count runs only on pages without a sheet: free on the normal path.)
    if no_sheet {
        let ink = omr_imaging::ink_coverage(&w.image);
        if ink.is_blank(BLANK_MAX_DARK_PPM) {
            return Staged {
                measured: None,
                refusal: Some(cap012(&t.page_id, ink.dark_ppm)),
                capture: Some(capture),
                source_dpi,
                working,
                blank: true,
                notes: Vec::new(),
            };
        }
    }
    // Effective resolution (§8.9 CAP-002): an own form's from its corner tags (scaled back
    // from the working copy to the scan), else the trusted PDF placement or the page size.
    let effective = match &measured {
        Measured::Own(m) => m.registration.as_ref().map(|a| {
            let long = w.image.width().max(w.image.height()).max(1);
            (a.dpi_eff() * decoded_long as f64 / long as f64).round() as u32
        }),
        Measured::Legacy(..) => None,
    }
    .or(d_source_dpi.filter(|x| (MIN_TRUSTED_DPI..=MAX_TRUSTED_DPI).contains(x)))
    .or(w.source_dpi);
    let mut notes = Vec::new();
    if let Some(e) = effective.and_then(|d| cap002(&t.page_id, d)) {
        if e.severity == Severity::Error {
            return Staged {
                measured: None,
                refusal: Some(e),
                capture: Some(capture),
                source_dpi,
                working,
                blank: false,
                notes,
            };
        }
        notes.push(e);
    }
    Staged {
        measured: Some(measured),
        refusal: None,
        capture: Some(capture),
        source_dpi,
        working,
        blank: false,
        notes,
    }
}

/// Questions a read page did not auto-accept (its abstentions).
fn abstentions(r: &PageRead) -> usize {
    r.questions.iter().filter(|q| !q.auto_accepted).count()
}

/// Two reads of one sheet agree (§7.4): every answer auto-accepted in both has the same
/// response, and the student-ID columns and the bubbled form agree wherever both read them
/// confidently. Abstentions never disagree.
fn reads_agree(a: &PageRead, b: &PageRead) -> bool {
    let by_q: BTreeMap<u16, &omr_model::QuestionOutcome> =
        a.questions.iter().map(|q| (q.question, q)).collect();
    let answers = b.questions.iter().all(|q| {
        by_q.get(&q.question)
            .is_none_or(|p| !(p.auto_accepted && q.auto_accepted) || p.response == q.response)
    });
    let id = match (&a.student_id, &b.student_id) {
        (Some(x), Some(y)) => {
            let (tx, ty): (Vec<char>, Vec<char>) =
                (x.text().chars().collect(), y.text().chars().collect());
            tx.len() == ty.len()
                && (0..tx.len()).all(|c| {
                    x.review.get(c).copied().unwrap_or(true)
                        || y.review.get(c).copied().unwrap_or(true)
                        || tx[c] == ty[c]
                })
        }
        _ => true,
    };
    let form = match (&a.form_choice, &b.form_choice) {
        (Some(x), Some(y)) => !(x.confident && y.confident) || x.response == y.response,
        _ => true,
    };
    answers && id && form
}

/// Copies of one sheet (in canonical order) grouped by agreement: each group is one reading
/// of the sheet, with the copy that is graded (the fewest abstentions, then the first in
/// canonical order, §7.4) and the rescans it supersedes (COL-003). Groups come in the order of
/// their first copy; copies in different groups disagree.
fn group_copies(pages: &[PageState], copies: &[usize]) -> Vec<(usize, Vec<usize>)> {
    let read = |i: usize| pages[i].read.as_ref().expect("read pages only");
    let mut reps: Vec<(usize, Vec<usize>)> = Vec::new();
    for &i in copies {
        match reps
            .iter()
            .position(|(r, _)| reads_agree(read(*r), read(i)))
        {
            Some(k) => {
                let (r, sup) = &mut reps[k];
                if abstentions(read(i)) < abstentions(read(*r)) {
                    sup.push(*r);
                    *r = i;
                } else {
                    sup.push(i);
                }
            }
            None => reps.push((i, Vec::new())),
        }
    }
    reps
}

/// The fully bubbled student number of a read page (every column one confident digit).
fn full_id(r: &PageRead) -> Option<String> {
    let g = r.student_id.as_ref()?;
    let t = g.text();
    (g.confident() && !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit())).then_some(t)
}

/// Collates read pages into booklets (§7.5): one page per booklet on single-page sheets;
/// own-form multi-page booklets by the verified serial, generic copies in input order.
/// Rescans are resolved first (§7.4): a second copy of a sheet (same verified serial and page,
/// or on serial-less single-page sheets the same fully bubbled student number) whose marks
/// agree with the first is superseded (COL-003, INFO: kept, linked, not graded); one whose
/// marks differ is held out with COL-004 (ERROR) and never overwrites or duplicates the copy
/// that is graded.
fn collate(
    def: &ExamDefinition,
    rec: &Recognizer,
    pages: &mut [PageState],
    tasks: &[PageTask],
) -> Vec<Booklet> {
    let n = usize::from(def.questions);
    let multi_page = rec.page_count() > 1;
    let page_no = |pages: &[PageState], i: usize| pages[i].read.as_ref().map_or(1, |r| r.page_no);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut held: Vec<(usize, u32)> = Vec::new();
    // (superseded page, the page graded instead).
    let mut superseded: Vec<(usize, usize)> = Vec::new();
    let mut by_serial: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut generic: Vec<usize> = Vec::new();
    let labels = rec.form_labels();
    // A verified page code naming a form this exam does not declare (form C of a sheet set
    // whose exam uses A and B) is an unknown form (IDN-002): its questions follow a key the
    // exam does not have, so it is never scored under another form's key or offered for one.
    for p in pages.iter_mut() {
        let id = p.page_id.clone();
        if p.status != PageStatus::Read {
            continue;
        }
        let Some(r) = p.read.as_mut() else {
            continue;
        };
        let Some(f) = r.code_form else {
            continue;
        };
        let lab = usize::from(f.0)
            .checked_sub(1)
            .and_then(|k| labels.get(k))
            .cloned()
            .unwrap_or_else(|| f.letter().to_string());
        if !def.forms.contains(&lab) {
            p.status = PageStatus::Unattributed;
            r.exceptions.push(
                Exception::must("IDN-002", Severity::Blocker, Scope::Page)
                    .with_target(Target {
                        page: Some(id),
                        booklet: r.code_serial,
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "the verified page code names form {lab}, which this exam does not \
                         declare ({}); not graded: add form {lab} and its key to the exam, or \
                         remove the sheet",
                        def.forms.join(", ")
                    )),
            );
        }
    }
    for (i, p) in pages.iter().enumerate() {
        if p.status != PageStatus::Read {
            continue;
        }
        match p.read.as_ref().and_then(|r| r.code_serial) {
            Some(s) if s > 0 => by_serial.entry(s).or_default().push(i),
            _ => generic.push(i),
        }
    }
    for (s, idx) in by_serial {
        let mut by_page: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        for i in idx {
            by_page.entry(page_no(pages, i)).or_default().push(i);
        }
        let mut g = Vec::new();
        for copies in by_page.values() {
            let mut reading = group_copies(pages, copies).into_iter();
            let (kept, sup) = reading.next().expect("at least one copy");
            g.push(kept);
            superseded.extend(sup.into_iter().map(|i| (i, kept)));
            // Copies that disagree with the graded one are held out (never a second booklet
            // with the same serial).
            for (r, sup) in reading {
                held.extend(std::iter::once(r).chain(sup).map(|i| (i, s)));
            }
        }
        g.sort_unstable();
        groups.push(g);
    }
    if !multi_page {
        let mut by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut single = Vec::new();
        for i in generic {
            match pages[i].read.as_ref().and_then(full_id) {
                Some(id) => by_id.entry(id).or_default().push(i),
                None => single.push(i),
            }
        }
        // One student number on sheets whose marks differ stays one booklet per reading, for
        // identity review (IDN-005).
        for copies in by_id.values() {
            for (kept, sup) in group_copies(pages, copies) {
                single.push(kept);
                superseded.extend(sup.into_iter().map(|i| (i, kept)));
            }
        }
        single.sort_unstable();
        groups.extend(single.into_iter().map(|i| vec![i]));
    } else {
        generic.sort_by_key(|&i| tasks[i].input_order);
        let mut cur: Vec<usize> = Vec::new();
        for i in generic {
            let pno = page_no(pages, i);
            if (pno <= 1 || cur.iter().any(|&c| page_no(pages, c) == pno)) && !cur.is_empty() {
                groups.push(std::mem::take(&mut cur));
            }
            cur.push(i);
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
    }
    // Canonical order: by the first page (pages are in content order).
    groups.sort_by_key(|g| g[0]);
    for &(i, serial) in &held {
        let p = &mut pages[i];
        p.status = PageStatus::Unattributed;
        let id = p.page_id.clone();
        if let Some(r) = p.read.as_mut() {
            r.exceptions.push(
                Exception::must("COL-004", Severity::Error, Scope::Booklet)
                    .with_sub_reason("marks_differ")
                    .with_target(Target {
                        page: Some(id),
                        booklet: Some(serial),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "booklet {serial} page {} was scanned again and the marks differ; the \
                         first copy is graded, this one is held out (keep it as not exam \
                         material, or rescan and grade again without the wrong copy)",
                        r.page_no
                    )),
            );
        }
    }
    let max_code_serial = groups
        .iter()
        .filter_map(|g| pages[g[0]].read.as_ref().and_then(|r| r.code_serial))
        .max()
        .unwrap_or(0);
    let mut booklets = Vec::with_capacity(groups.len());
    for (rank, mut g) in groups.into_iter().enumerate() {
        g.sort_by_key(|&i| pages[i].read.as_ref().map_or(0, |r| r.page_no));
        let code_serial = pages[g[0]]
            .read
            .as_ref()
            .and_then(|r| r.code_serial)
            .filter(|&s| s > 0);
        let serial = code_serial.unwrap_or(max_code_serial + rank as u32 + 1);
        let bi = booklets.len();
        let mut questions: Vec<Option<QuestionState>> = vec![None; n];
        let mut exceptions = Vec::new();
        let mut id_read = None;
        let (mut sheet_section, mut exam_type) = (String::new(), String::new());
        let (mut declared_form, mut declared_text, mut form_source) =
            (None, String::new(), FormSource::Unknown);
        for &pi in &g {
            pages[pi].booklet = Some(bi);
            let Some(r) = pages[pi].read.as_ref() else {
                continue;
            };
            for q in &r.questions {
                let qi = usize::from(q.question);
                if qi < n {
                    let mut q = q.clone();
                    // A mark on an option the sheet prints but the exam does not use (E on a
                    // four-option exam on the IAU sheet) never reaches scoring as an option it
                    // cannot score, and is never dropped: the question goes to review.
                    if let Some(e) = option_not_offered(def, serial_hint(&r.code_serial), &q) {
                        q.auto_accepted = false;
                        q.exceptions.push(e);
                    }
                    // A phone photo of a student's sheet (§8.10 step 7: the phone slice is
                    // not calibrated): the reading is proposed, never accepted on its own.
                    if r.photo && q.auto_accepted {
                        q.auto_accepted = false;
                        q.exceptions.push(uncalibrated_capture(
                            serial_hint(&r.code_serial),
                            q.question,
                        ));
                    }
                    let status = if q.auto_accepted {
                        ResponseStatus::Auto
                    } else {
                        ResponseStatus::Pending
                    };
                    questions[qi] = Some(QuestionState {
                        current: q.response.clone(),
                        recognized: q,
                        status,
                        page: pi,
                    });
                }
            }
            for e in &r.exceptions {
                if e.code == "MRK-015" {
                    exceptions.push(e.clone());
                }
            }
            if id_read.is_none() {
                id_read = r.student_id.clone();
            }
            // The section and exam type bubbled on the sheet, when read confidently (digits
            // without a leading or trailing blank column).
            if sheet_section.is_empty()
                && let Some(g) = r.section.as_ref().filter(|g| g.confident())
            {
                sheet_section = g.text().trim_matches('_').to_string();
                if sheet_section.contains('_') {
                    sheet_section.clear();
                }
            }
            if exam_type.is_empty()
                && let Some(v) = r.exam_type.as_ref().and_then(|c| c.value())
            {
                exam_type = v.to_string();
            }
            if let Some(f) = r.code_form {
                // The verified code names a printed form: map it by its letter, never by
                // position (the definition may list the forms in another order). A form the
                // exam does not declare was held out above (IDN-002).
                let lab = usize::from(f.0)
                    .checked_sub(1)
                    .and_then(|k| labels.get(k))
                    .cloned()
                    .unwrap_or_else(|| f.letter().to_string());
                declared_text = lab.clone();
                if let Some(k) = def.forms.iter().position(|x| *x == lab) {
                    declared_form = Some(k);
                    form_source = FormSource::Code;
                }
            } else if declared_form.is_none()
                && let Some(c) = &r.form_choice
            {
                match (&c.response, c.confident) {
                    (Response::Single(o), true) => {
                        let lab = labels.get(usize::from(*o)).cloned().unwrap_or_default();
                        declared_text = lab.clone();
                        if let Some(k) = def.forms.iter().position(|f| *f == lab) {
                            declared_form = Some(k);
                            form_source = FormSource::Bubbled;
                        }
                    }
                    (Response::Blank, true) => declared_text = String::new(),
                    _ => declared_text = "?".into(),
                }
            }
        }
        let questions: Vec<QuestionState> = questions
            .into_iter()
            .enumerate()
            .map(|(q, s)| {
                s.unwrap_or_else(|| QuestionState {
                    recognized: omr_model::QuestionOutcome {
                        question: q as u16,
                        response: Response::Unreadable { bubbles: vec![] },
                        auto_accepted: false,
                        logit: omr_model::LogitMilli(0),
                        exceptions: vec![
                            Exception::must("COL-001", Severity::Error, Scope::Booklet)
                                .with_target(Target {
                                    booklet: Some(serial),
                                    question: Some(q as u16),
                                    ..Target::default()
                                })
                                .with_detail("the page holding this question is missing"),
                        ],
                    },
                    current: Response::Unreadable { bubbles: vec![] },
                    status: ResponseStatus::Pending,
                    page: g[0],
                })
            })
            .collect();
        let read_text = id_read
            .as_ref()
            .map(|g: &omr_recog::GridField| g.text())
            .unwrap_or_default();
        let single = def.forms.len() == 1;
        // The version backstop (§10.3): a confident letter the exam does not declare is held,
        // never scored under another version's key, on a one-version exam too, where only a
        // blank, unclear or the declared letter takes the one version (as an own sheet's
        // undeclared page code is, IDN-002). Definitions saved before the backstop keep the
        // one-version shortcut.
        let undeclared = def.hold_undeclared_versions
            && declared_form.is_none()
            && !declared_text.is_empty()
            && declared_text != "?";
        let (form, form_source) = if undeclared {
            exceptions.push(infer::sco005_undeclared(serial, &declared_text, &def.forms));
            (None, FormSource::Unknown)
        } else if single && declared_form.is_none() {
            let source = if def.hold_undeclared_versions {
                FormSource::Shared
            } else {
                FormSource::Bubbled
            };
            (Some(0), source)
        } else if single {
            (Some(0), form_source)
        } else {
            (declared_form, form_source)
        };
        // A blank booklet (MRK-015) is never scored 0 automatically: its score is held as
        // pending until a reviewer acknowledges it (§8.8).
        let hold = exceptions
            .iter()
            .find(|e| e.code == "MRK-015" && e.severity >= Severity::Error)
            .map(|e| e.code.clone());
        // A printed serial names the booklet; a sheet without one is named by its first page
        // (content-derived), so the name survives a later batch that changes the ranks.
        let booklet_id = match code_serial {
            Some(s) => format!("bk-{s:06}"),
            None => format!("bk-{}", pages[g[0]].page_id.trim_start_matches("pg-")),
        };
        booklets.push(Booklet {
            booklet_id,
            serial,
            pages: g,
            declared_form,
            declared_text,
            form,
            form_source,
            inference: None,
            hold,
            questions,
            identity: IdentityState {
                read: read_text,
                student: None,
                status: IdentityStatus::Review,
                candidates: Vec::new(),
                exceptions: Vec::new(),
                confirmed: false,
            },
            exceptions,
            sheet_section,
            exam_type,
        });
    }
    for &(i, kept) in &superseded {
        let serial = pages[kept].booklet.map(|b| booklets[b].serial);
        let kept_id = pages[kept].page_id.clone();
        let p = &mut pages[i];
        p.status = PageStatus::Duplicate;
        p.booklet = None;
        let e = Exception::must("COL-003", Severity::Info, Scope::Booklet)
            .with_target(Target {
                page: Some(p.page_id.clone()),
                booklet: serial,
                ..Target::default()
            })
            .with_detail(format!(
                "rescan of the sheet on page {kept_id}: the marks agree, so the copy with fewer \
                 abstentions is graded and this one is kept, linked and not graded"
            ));
        p.refusal = Some(e);
    }
    booklets
}

fn serial_hint(code_serial: &Option<u32>) -> Option<u32> {
    code_serial.filter(|&s| s > 0)
}

/// Sub-reason of the batch CAP-003 WARN that says a batch holds phone photos (§8.10).
pub const PHONE_UNCALIBRATED: &str = "phone_uncalibrated";

/// MRK-001 `uncalibrated_capture` (ERROR, Item): an answer on a phone photo of a student's
/// sheet, sent to review because the phone slice is not calibrated (§8.10 step 7).
fn uncalibrated_capture(booklet: Option<u32>, question: u16) -> Exception {
    Exception::must("MRK-001", Severity::Error, Scope::Item)
        .with_sub_reason("uncalibrated_capture")
        .with_target(Target {
            booklet,
            question: Some(question),
            ..Target::default()
        })
        .with_detail(
            "phone photo: the phone capture is not calibrated (§8.10 step 7), so no answer on \
             it is accepted on its own; confirm the answer shown (Enter) or correct it from the \
             crop, or rescan the sheet on a scanner",
        )
}

/// MRK-017 `option_not_offered` (ERROR, Item) when a response marks an option at or beyond the
/// exam's option count: the sheet prints it, the exam does not use it.
pub fn option_not_offered(
    def: &ExamDefinition,
    booklet: Option<u32>,
    q: &omr_model::QuestionOutcome,
) -> Option<Exception> {
    let m = def.options;
    let outside: Vec<u8> = match &q.response {
        Response::Single(o) => vec![*o],
        Response::Multi(v) | Response::Set(v) => v.clone(),
        _ => Vec::new(),
    }
    .into_iter()
    .filter(|&o| o >= m)
    .collect();
    if outside.is_empty() {
        return None;
    }
    let letter = |o: u8| char::from(b'A' + o.min(25)).to_string();
    let marked: Vec<String> = outside.iter().map(|&o| letter(o)).collect();
    Some(
        Exception::must("MRK-017", Severity::Error, Scope::Item)
            .with_sub_reason("option_not_offered")
            .with_target(Target {
                booklet,
                question: Some(q.question),
                ..Target::default()
            })
            .with_detail(format!(
                "option {} is marked, but the exam uses options A–{} (--options {m}): choose \
                 the answer among A–{}, or blank for no valid answer (scored as no answer); if \
                 the exam really has more options, grade it again with --options set to them",
                marked.join(", "),
                letter(m - 1),
                letter(m - 1)
            )),
    )
}

/// Roster snapshot of a definition.
pub fn roster_snapshot(def: &ExamDefinition) -> Result<Option<RosterSnapshot>, AppError> {
    let Some(rows) = &def.roster else {
        return Ok(None);
    };
    crate::exam::check_roster(rows, &def.id_scheme)?;
    let records = rows
        .iter()
        .map(|r| RosterRecord {
            student_key: r.id.clone(),
            key_from_numeric_cell: r.numeric_cell,
            name: r.name.clone(),
            name_ar: (!r.name_ar.is_empty()).then(|| r.name_ar.clone()),
            name_en: None,
            section: (!r.section.is_empty()).then(|| r.section.clone()),
            campus: None,
            track: None,
            sis_status: None,
        })
        .collect();
    let scheme = def.id_scheme.scheme();
    match RosterSnapshot::build(records, &scheme, &ImportOptions::default()) {
        Ok((s, _)) => Ok(Some(s)),
        Err(e) => Err(AppError::Input(format!("roster: {e}"))),
    }
}

/// Identity for every booklet: roster-constrained or rosterless decoding, then the batch
/// assignment (duplicates, not-on-roster).
fn identify(
    def: &ExamDefinition,
    booklets: &mut [Booklet],
    pages: &[PageState],
) -> Result<(), AppError> {
    let scheme = def.id_scheme.scheme();
    let policy = IdentityPolicy::default();
    let roster = roster_snapshot(def)?;
    let prior = PriorContext::default();
    let items: Vec<BookletIdentity> = booklets
        .iter()
        .map(|b| {
            let grid = b
                .pages
                .iter()
                .find_map(|&p| pages[p].read.as_ref().and_then(|r| r.student_id.clone()))
                .map(|g| g.read);
            let ev = BookletEvidence {
                grid,
                typed: None,
                bound_student: None,
            };
            let decision = match &roster {
                Some(r) => omr_identity::decode_booklet(&ev, r, &scheme, &prior, &policy),
                None => omr_identity::decode_rosterless(&ev, &scheme, &policy),
            };
            BookletIdentity {
                booklet: b.booklet_id.clone(),
                decision,
                confirmed: None,
            }
        })
        .collect();
    let outcome = match &roster {
        Some(r) => omr_identity::assign_batch(&items, r, &prior, &policy),
        None => omr_identity::assign_rosterless(&items),
    };
    let by_id: BTreeMap<String, usize> = booklets
        .iter()
        .enumerate()
        .map(|(i, b)| (b.booklet_id.clone(), i))
        .collect();
    for a in &outcome.booklets {
        let Some(&bi) = by_id.get(&a.booklet) else {
            continue;
        };
        let b = &mut booklets[bi];
        let student = match &a.assigned {
            Assigned::Student(k) | Assigned::Duplicate(k) => Some(k.0.clone()),
            Assigned::NotOnRoster => None,
        };
        let mut exceptions: Vec<Exception> = a
            .decision
            .exceptions
            .iter()
            .cloned()
            .map(|mut e| {
                e.target.booklet = Some(b.serial);
                e
            })
            .collect();
        if let Some(k) = &student {
            for (dk, e) in &outcome.duplicates {
                if &dk.0 == k {
                    let mut e = e.clone();
                    e.target.booklet = Some(b.serial);
                    exceptions.push(e);
                }
            }
        }
        // The section bubbled on the sheet against the roster's (§9.9 row 11): surfaced,
        // never deciding (INFO).
        if let (Some(k), Some(rows)) = (&student, &def.roster)
            && !b.sheet_section.is_empty()
            && let Some(r) = rows
                .iter()
                .find(|r| omr_identity::normalize_id(&r.id) == *k)
            && !r.section.is_empty()
            && !same_section(&r.section, &b.sheet_section)
        {
            exceptions.push(
                Exception::must("IDN-004", Severity::Info, Scope::Booklet)
                    .with_sub_reason("cross_section")
                    .with_target(Target {
                        booklet: Some(b.serial),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "the sheet bubbles section {} but the roster lists the student in \
                         section {}",
                        b.sheet_section, r.section
                    )),
            );
        }
        b.identity = IdentityState {
            read: a
                .decision
                .read
                .grid
                .clone()
                .unwrap_or_else(|| b.identity.read.clone()),
            student,
            status: a.status,
            candidates: a
                .decision
                .candidates
                .iter()
                .take(5)
                .map(|c| (c.student.0.clone(), c.posterior))
                .collect(),
            exceptions,
            confirmed: false,
        };
    }
    Ok(())
}

/// Two section codes name the same section: equal ignoring case and surrounding blanks, or
/// equal as numbers (`071` and `71`).
fn same_section(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The effective key of per-form keys (§10.1): cells in the definition's form order, the
/// definition's points; identical keys share master items.
pub fn provided_key(
    def: &ExamDefinition,
    forms: &[crate::exam::FormKey],
    version: u32,
) -> Result<EffectiveKey, AppError> {
    let (points, form_points) = EffectiveKey::points_from(def);
    let mut cells = Vec::new();
    for f in &def.forms {
        let k = forms.iter().find(|k| &k.form == f).ok_or_else(|| {
            AppError::Definition(format!("no key for form {f}; add its column or row"))
        })?;
        cells.push(
            k.answers
                .iter()
                .map(|a| {
                    parse_key_entry(a, def.options)
                        .map(KeyCell::from)
                        .unwrap_or(KeyCell::Missing)
                })
                .collect::<Vec<_>>(),
        );
    }
    // Forms share master items only when they share the key, the points and the mapping.
    let shared =
        cells.iter().all(|c| *c == cells[0]) && form_points.is_empty() && !def.has_form_mapping();
    Ok(EffectiveKey {
        version,
        inferred: false,
        forms: cells,
        points,
        form_points,
        shared_items: shared,
    })
}

/// The key in force, form inference and the related exceptions (§10.3). Versions read from
/// key sheets that are not confirmed take the key sheets' rows (§10.1; held, never scored until
/// the instructor confirms them); form inference of undeclared booklets uses them as a
/// proposal only (refreshed at confirmation).
fn resolve_key(
    def: &ExamDefinition,
    booklets: &mut [Booklet],
    exam_exceptions: &mut Vec<Exception>,
    key_sheets: &crate::model::KeySheets,
) -> Result<Option<EffectiveKey>, AppError> {
    let n = usize::from(def.questions);
    let (points, form_points) = EffectiveKey::points_from(def);
    let key = match &def.key {
        KeySource::None => None,
        KeySource::Provided { forms } => Some(provided_key(def, forms, 1)?),
        KeySource::Inferred => {
            let cells = infer::infer_key(booklets, def.forms.len(), n, def.options);
            exam_exceptions.push(crate::score::inferred_key_exception());
            Some(EffectiveKey {
                version: 1,
                inferred: true,
                shared_items: def.forms.len() == 1,
                forms: cells,
                points,
                form_points,
            })
        }
    };
    let mut key = key;
    crate::keysheet::overlay(def, key_sheets, &mut key, exam_exceptions);
    if def.forms.len() < 2 {
        return Ok(key);
    }
    let Some(k) = &key else {
        // Several forms, no key: booklets without a declared form cannot be scored or inferred.
        return Ok(None);
    };
    // Identical versions (the same key, points and mapping on every one, §10.3): a booklet
    // that declares no version is scored under the shared key and counted (SCO-005
    // `identical_forms`, INFO), never queued. Not while key sheets wait for confirmation, which
    // may still make the versions differ, and not in definitions saved before the backstops.
    let identical =
        def.hold_undeclared_versions && !k.inferred && k.shared_items && !key_sheets.pending;
    let inf = infer::infer_forms(booklets, &k.forms, n, def.options);
    for (b, inf) in booklets.iter_mut().zip(inf) {
        b.inference = inf.clone();
        // A held sheet (an undeclared version letter) gets no provisional form and no second
        // SCO-005: its inference is only the reviewer's suggestion (§10.3 backstop).
        if b.undeclared_version() {
            continue;
        }
        match (b.declared_form, inf) {
            (Some(d), Some(i)) => {
                if i.argmax != d {
                    let delta = (i.ll_milli[i.argmax] - i.ll_milli[d]) as f64 / 1000.0;
                    if delta >= infer::DELTA_LL_NATS {
                        b.exceptions.push(infer::idn003(
                            b.serial,
                            &def.forms[d],
                            &def.forms[i.argmax],
                            delta,
                        ));
                    }
                }
            }
            (None, _) if identical => {
                b.form = Some(0);
                b.form_source = FormSource::Shared;
                b.exceptions.push(infer::sco005_identical(b.serial));
            }
            (None, Some(i)) => {
                b.form = Some(i.argmax);
                b.form_source = FormSource::Inferred;
                b.exceptions.push(infer::sco005_inferred(
                    b.serial,
                    &def.forms[i.argmax],
                    i.margin_milli as f64 / 1000.0,
                ));
            }
            (None, None) => {
                b.exceptions.push(
                    Exception::must("SCO-005", Severity::Error, Scope::Booklet)
                        .with_sub_reason("inferred")
                        .with_target(Target {
                            booklet: Some(b.serial),
                            ..Target::default()
                        })
                        .with_detail("no form declared and no usable responses to infer it"),
                );
            }
            (Some(_), None) => {}
        }
    }
    let _ = ParsedKey::Missing;
    Ok(key)
}

/// Shared parallel helper for benchmarks: runs only decode + page stage + batch stage on
/// sources in memory and returns (pages, auto-accepted questions, questions).
pub fn recognize_only(
    def: &ExamDefinition,
    sources: &[SourceFile],
    pool: &rayon::ThreadPool,
) -> Result<(usize, usize, usize), AppError> {
    let rec = Recognizer::new(def)?;
    let opened: Vec<(OpenSource, u32)> =
        pool.install(|| sources.par_iter().map(ingest::open_source).collect());
    let mut tasks = Vec::new();
    for (s, (_, n)) in opened.iter().enumerate() {
        for i in 0..*n {
            tasks.push(PageTask {
                page_id: ingest::page_id(&sources[s].sha384, i),
                source: s,
                index: i,
                duplicate_of: None,
                input_order: s,
            });
        }
    }
    let staged: Vec<Staged> = pool.install(|| {
        tasks
            .par_iter()
            .map(|t| stage_page(&rec, Some(&sources[t.source]), &opened[t.source].0, t))
            .collect()
    });
    let ms_: Vec<&Measured> = staged.iter().filter_map(|s| s.measured.as_ref()).collect();
    let b = rec.baseline(&ms_);
    let counts = Mutex::new((0usize, 0usize));
    pool.install(|| {
        staged.par_iter().zip(tasks.par_iter()).for_each(|(s, t)| {
            if let Some(m) = &s.measured {
                let r = rec.decide(m, &b, &t.page_id);
                let a = r.questions.iter().filter(|q| q.auto_accepted).count();
                let mut c = counts.lock().expect("counts");
                c.0 += a;
                c.1 += r.questions.len();
            }
        })
    });
    let (a, q) = counts.into_inner().expect("counts");
    Ok((tasks.len(), a, q))
}
