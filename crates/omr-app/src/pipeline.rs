//! The grading pipeline and its scheduler (§4.5, §8.1, §21.2). The scheduler owns the rayon
//! pool (R4): pages are decoded and measured in parallel (one task per page), then the batch
//! stage runs per batch slice (capture class × device resolution) in page-ID order, identity
//! and scoring follow, and everything lands in the project store. Outputs do not depend on the
//! thread count or the input order (A5): pages are ordered by content (source SHA-384, page
//! index) and batch statistics are order-independent.

use crate::AppError;
use crate::engine::{Measured, PageRead, Recognizer};
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
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

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
}

impl Default for GradeOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            project_dir: None,
            snapshot_utc: None,
            operator: "principal:operator".into(),
            max_pages: None,
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

/// Grades sources already in memory (tests, benchmarks).
pub fn grade_sources(
    def: &ExamDefinition,
    sources: Vec<Result<SourceFile, (PathBuf, String)>>,
    opts: &GradeOptions,
    pool: &rayon::ThreadPool,
    t0: Instant,
    progress: &(dyn Fn(&Progress) + Sync),
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
            Ok(f) => files.push((k, f)),
            Err((p, e)) => broken.push((k, p, e)),
        }
    }
    // Byte-identical files: the original is the first by name and path, never by input
    // position, so the same set of files yields the same outputs in any order (A5).
    files.sort_by(|a, b| {
        a.1.sha384
            .cmp(&b.1.sha384)
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
    }
    for (k, p, e) in broken {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
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
            .map(|(f, dup)| match (f, dup) {
                (Some(_), Some(_)) => (OpenSource::Broken("duplicate".into()), 0),
                (Some(f), None) => ingest::open_source(f),
                (None, _) => (OpenSource::Broken("file could not be read".into()), 1),
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

    // ---- Spool first (§7.4): originals durable in the store before any computation.
    let mut store = match &opts.project_dir {
        Some(dir) => Some(StoreWriter::create(
            dir,
            def,
            &snapshot_utc,
            &opts.operator,
        )?),
        None => None,
    };
    if let Some(st) = store.as_mut() {
        st.write_setup(def, recognizer.manifest())?;
        let blake = pool.install(|| st.stage_sources(&source_files, &dup_of))?;
        for (s, b) in blake.into_iter().enumerate() {
            source_infos[s].blake3 = b;
        }
        st.spool_pages(
            &tasks
                .iter()
                .map(|t| (t.page_id.clone(), t.source, t.index, t.duplicate_of))
                .collect::<Vec<_>>(),
            &source_infos,
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
                let out = if t.duplicate_of.is_some() {
                    Staged {
                        measured: None,
                        refusal: None,
                        capture: None,
                        source_dpi: None,
                        working: None,
                    }
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

    // ---- Batch stage: baseline per slice (capture class × device dpi), then decisions.
    progress(&Progress::Stage("batch"));
    let t_batch = Instant::now();
    let mut slices: BTreeMap<(bool, Option<u32>), Vec<usize>> = BTreeMap::new();
    for (i, s) in staged.iter().enumerate() {
        if let Some(m) = &s.measured {
            slices
                .entry((m.bitonal(), s.source_dpi))
                .or_default()
                .push(i);
        }
    }
    let mut baselines = BTreeMap::new();
    let mut batch_exceptions: Vec<Exception> = Vec::new();
    for (key, idx) in &slices {
        let ms_: Vec<&Measured> = idx
            .iter()
            .filter_map(|&i| staged[i].measured.as_ref())
            .collect();
        let b = recognizer.baseline(&ms_);
        for e in b.exceptions() {
            push_batch(&mut batch_exceptions, e);
        }
        baselines.insert(*key, b);
    }
    // A panic while deciding one page quarantines that page (CAP-001), never the batch.
    let reads: Vec<Result<Option<PageRead>, ()>> = pool.install(|| {
        staged
            .par_iter()
            .zip(tasks.par_iter())
            .map(|(s, t)| {
                let Some(m) = s.measured.as_ref() else {
                    return Ok(None);
                };
                let b = &baselines[&(m.bitonal(), s.source_dpi)];
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    recognizer.decide(m, b, &t.page_id)
                }))
                .map(Some)
                .map_err(|_| ())
            })
            .collect()
    });
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
        let status = if t.duplicate_of.is_some() {
            PageStatus::Duplicate
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

    // ---- Collation: booklets in canonical order.
    let mut booklets = collate(def, &recognizer, &mut pages, &tasks);

    // ---- Identity (§9).
    progress(&Progress::Stage("identity"));
    let t_id = Instant::now();
    identify(def, &mut booklets, &pages)?;
    timings.identity_ms = ms(t_id);

    // ---- Forms, key, inference, scoring (§10).
    progress(&Progress::Stage("scoring"));
    let t_sc = Instant::now();
    let mut exam_exceptions = Vec::new();
    let key = resolve_key(def, &mut booklets, &mut exam_exceptions)?;
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
    };
    let scoring = Scoring::build(&session)?;
    timings.scoring_ms = ms(t_sc);

    // ---- Review queue.
    let labels = def.option_labels();
    let digits: Vec<String> = (0..10).map(|d| d.to_string()).collect();
    session.review.items = build_queue(&session, &labels, &digits);

    // ---- Store: recognition run, identity, exceptions, score run.
    progress(&Progress::Stage("store"));
    let t_st = Instant::now();
    if let Some(st) = store.as_mut() {
        st.write_results(&mut session, scoring.as_ref())?;
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
    };
    let Some(src) = src else {
        return none(ingest::cap001(&t.page_id, "file could not be read"));
    };
    let d = match ingest::decode_page(src, open, t.index, &t.page_id) {
        Ok(d) => d,
        Err(e) => return none(e),
    };
    let capture = d.capture;
    let w = rec.to_working(d.image, d.capture, d.known_dpi);
    let working = Some((w.image.width() as u32, w.image.height() as u32));
    let source_dpi = d.source_dpi.or(w.source_dpi);
    let measured = rec.measure(&w, &t.page_id);
    Staged {
        measured: Some(measured),
        refusal: None,
        capture: Some(capture),
        source_dpi,
        working,
    }
}

/// Collates read pages into booklets (§7.5): one page per booklet on single-page sheets;
/// own-form multi-page booklets by the verified serial, generic copies in input order.
fn collate(
    def: &ExamDefinition,
    rec: &Recognizer,
    pages: &mut [PageState],
    tasks: &[PageTask],
) -> Vec<Booklet> {
    let n = usize::from(def.questions);
    let multi_page = rec.page_count() > 1;
    let page_no = |pages: &[PageState], i: usize| pages[i].read.as_ref().map_or(1, |r| r.page_no);
    // Groups of page indices. A second page with the same verified serial and template page
    // (a rescan that is not byte-identical) is held out with COL-004: it never overwrites the
    // first copy (canonical order) and never becomes a second booklet with the same serial.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut held: Vec<(usize, u32)> = Vec::new();
    let mut by_serial: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut generic: Vec<usize> = Vec::new();
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
        let mut seen = std::collections::BTreeSet::new();
        let mut g = Vec::new();
        for i in idx {
            if seen.insert(page_no(pages, i)) {
                g.push(i);
            } else {
                held.push((i, s));
            }
        }
        groups.push(g);
    }
    if !multi_page {
        groups.extend(generic.into_iter().map(|i| vec![i]));
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
                    .with_target(Target {
                        page: Some(id),
                        booklet: Some(serial),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "booklet {serial} page {} was scanned again with different content; \
                         the first copy is graded, this one is held out",
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
    let labels = rec.form_labels();
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
        let (mut declared_form, mut declared_text, mut form_source) =
            (None, String::new(), FormSource::Unknown);
        let mut code_unknown = false;
        for &pi in &g {
            pages[pi].booklet = Some(bi);
            let Some(r) = pages[pi].read.as_ref() else {
                continue;
            };
            for q in &r.questions {
                let qi = usize::from(q.question);
                if qi < n {
                    let status = if q.auto_accepted {
                        ResponseStatus::Auto
                    } else {
                        ResponseStatus::Pending
                    };
                    questions[qi] = Some(QuestionState {
                        recognized: q.clone(),
                        current: q.response.clone(),
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
            if let Some(f) = r.code_form {
                // The verified code names a printed form: map it by its letter, never by
                // position (the definition may list the forms in another order).
                let lab = usize::from(f.0)
                    .checked_sub(1)
                    .and_then(|k| labels.get(k))
                    .cloned()
                    .unwrap_or_else(|| f.letter().to_string());
                declared_text = lab.clone();
                match def.forms.iter().position(|x| *x == lab) {
                    Some(k) => {
                        declared_form = Some(k);
                        form_source = FormSource::Code;
                        code_unknown = false;
                    }
                    None => {
                        declared_form = None;
                        code_unknown = true;
                    }
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
        let single = def.forms.len() == 1 && !code_unknown;
        if code_unknown && def.forms.len() == 1 {
            // Several forms are inferred and reviewed in `resolve_key`; a one-form exam has
            // nothing to infer, so the booklet waits for a reviewer's form decision.
            exceptions.push(
                Exception::must("SCO-005", Severity::Error, Scope::Booklet)
                    .with_sub_reason("code_form_not_in_exam")
                    .with_target(Target {
                        booklet: Some(serial),
                        ..Target::default()
                    })
                    .with_detail(format!(
                        "the verified page code names form {declared_text}, which this exam does \
                         not declare ({}); not scored until a reviewer decides",
                        def.forms.join(", ")
                    )),
            );
        }
        // A blank booklet (MRK-015) is never scored 0 automatically: its score is held as
        // pending until a reviewer acknowledges it (§8.8).
        let hold = exceptions
            .iter()
            .find(|e| e.code == "MRK-015" && e.severity >= Severity::Error)
            .map(|e| e.code.clone());
        booklets.push(Booklet {
            booklet_id: format!("bk-{serial:06}"),
            serial,
            pages: g,
            declared_form,
            declared_text,
            form: if single { Some(0) } else { declared_form },
            form_source: if single && declared_form.is_none() {
                FormSource::Bubbled
            } else {
                form_source
            },
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
        });
    }
    booklets
}

/// Roster snapshot of a definition.
pub fn roster_snapshot(def: &ExamDefinition) -> Result<Option<RosterSnapshot>, AppError> {
    let Some(rows) = &def.roster else {
        return Ok(None);
    };
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

/// The key in force, form inference and the related exceptions (§10.3).
fn resolve_key(
    def: &ExamDefinition,
    booklets: &mut [Booklet],
    exam_exceptions: &mut Vec<Exception>,
) -> Result<Option<EffectiveKey>, AppError> {
    let n = usize::from(def.questions);
    let points = if def.points.is_empty() {
        vec!["1".to_string(); n]
    } else {
        def.points.clone()
    };
    let key = match &def.key {
        KeySource::None => None,
        KeySource::Provided { forms } => {
            let mut cells = Vec::new();
            for f in &def.forms {
                let k = forms
                    .iter()
                    .find(|k| &k.form == f)
                    .ok_or_else(|| AppError::Definition(format!("no key for form {f}")))?;
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
            let shared = cells.iter().all(|c| *c == cells[0]);
            Some(EffectiveKey {
                version: 1,
                inferred: false,
                forms: cells,
                points,
                shared_items: shared,
            })
        }
        KeySource::Inferred => {
            let cells = infer::infer_key(booklets, def.forms.len(), n, def.options);
            exam_exceptions.push(crate::score::inferred_key_exception());
            Some(EffectiveKey {
                version: 1,
                inferred: true,
                shared_items: def.forms.len() == 1,
                forms: cells,
                points,
            })
        }
    };
    if def.forms.len() < 2 {
        return Ok(key);
    }
    let Some(k) = &key else {
        // Several forms, no key: booklets without a declared form cannot be scored or inferred.
        return Ok(None);
    };
    let inf = infer::infer_forms(booklets, &k.forms, n, def.options);
    for (b, inf) in booklets.iter_mut().zip(inf) {
        b.inference = inf.clone();
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
