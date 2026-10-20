//! The open exam: review actions with audited events and incremental rescoring, undo, key
//! changes, derived analyses and persistence of the session snapshot (§4.7, §13.3, §10.1).

use crate::AppError;
use crate::engine::Recognizer;
use crate::exam::{ExamDefinition, parse_key_entry};
use crate::ingest::{self, SourceFile};
use crate::model::{FormSource, KeyCell, ResponseStatus, Session};
use crate::outcomes::OutcomeReport;
use crate::pipeline::{GradeOptions, Graded, Progress};
use crate::review::{
    Action, Change, Decision, ItemKind, ItemState, Lane, ReviewItem, ReviewReason, ReviewRecord,
};
use crate::score::Scoring;
use crate::store::{
    PAGE_CODE_KEY_FILE, PAGES_FILE, SECRETS_DIR, SESSION_FILE, SnapshotBinding, StoreWriter,
    store_reason,
};
use omr_analytics::{AnalysisData, AnalyticsConfig, AnalyticsReport};
use omr_imaging::GrayImage;
use omr_model::{Response, Severity};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Re-infers the form of booklets whose form came from inference (SCO-005 `inferred`) under
/// `key` (§10.3) and refreshes the proposal of their open form items.
fn refresh_inferred_forms(session: &mut Session, key: &crate::model::EffectiveKey) {
    let def = &session.definition;
    if def.forms.len() < 2 {
        return;
    }
    let inf = crate::infer::infer_forms(
        &session.booklets,
        &key.forms,
        usize::from(def.questions),
        def.options,
    );
    for (bi, i) in inf.into_iter().enumerate() {
        let b = &mut session.booklets[bi];
        // A held sheet (an undeclared version letter) keeps no form: only its suggestion
        // follows the new key, and only when the answers single a version out by 10 nats.
        if b.form.is_none() && b.undeclared_version() {
            let suggestion = i
                .as_ref()
                .filter(|i| crate::infer::confident(i))
                .map(|i| Decision::Form(i.argmax as u8));
            for it in session.review.items.iter_mut() {
                if it.booklet == Some(bi) && it.kind == ItemKind::Form && it.is_open() {
                    it.suggestion = suggestion.clone();
                }
            }
            b.inference = i;
            continue;
        }
        if b.form_source != FormSource::Inferred {
            continue;
        }
        if let Some(i) = i {
            b.form = Some(i.argmax);
            for it in session.review.items.iter_mut() {
                if it.booklet == Some(bi) && it.kind == ItemKind::Form && it.is_open() {
                    it.suggestion = Some(Decision::Form(i.argmax as u8));
                }
            }
            b.inference = Some(i);
        }
    }
}

/// Result of [`ExamProject::apply`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    /// Items closed or moved by the action.
    pub affected: Vec<u64>,
    /// The next item in the same lane, in queue order.
    pub next: Option<u64>,
}

/// An open, graded exam.
pub struct ExamProject {
    /// The exam state.
    pub session: Session,
    /// Scoring (`None` without a key).
    pub scoring: Option<Scoring>,
    store: Option<StoreWriter>,
    /// The snapshot on disk as last saved or verified: a save rewrites only what changed.
    bound: Option<Bound>,
    /// Booklets changed since the last save (their chunks are rewritten), or every booklet.
    dirty: Dirty,
    sources: Mutex<BTreeMap<usize, Arc<SourceFile>>>,
    pages: Mutex<Vec<(usize, Arc<GrayImage>)>>,
    recognizer: Mutex<Option<Arc<Recognizer>>>,
    /// The flag-stability job's result (§12.2) for the score run it was computed on.
    stability: Mutex<Option<omr_analytics::StabilityReport>>,
}

/// Booklets per snapshot chunk file (`booklets-NNNNN.json`): a review action rewrites the
/// session header and one chunk, not the whole exam (CR-14).
pub const BOOKLET_CHUNK: usize = 64;

/// The chunk index (`booklets.json`): the SHA-384 of every chunk file, in order.
pub const BOOKLET_INDEX_FILE: &str = "booklets.json";

fn chunk_name(c: usize) -> String {
    format!("booklets-{c:05}.json")
}

/// SHA-384 (hex) of each snapshot file as saved or verified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Bound {
    session: String,
    pages: String,
    index: String,
    chunks: Vec<String>,
}

/// Which booklets changed since the last save.
#[derive(Clone, Debug, Default)]
struct Dirty {
    all: bool,
    booklets: std::collections::BTreeSet<usize>,
}

/// A temporary snapshot file beside `name` (written, synced, then renamed into place).
fn tmp_name(name: &str) -> String {
    format!("{name}.tmp")
}

/// Reads one snapshot file and checks it against its recorded SHA-384. A file whose save was
/// interrupted after its event (the temporary copy holds the recorded bytes) is completed:
/// renamed into place when `recover`, else read from the temporary copy.
fn read_bound(
    cache: &Path,
    name: &str,
    want: Option<&str>,
    recover: bool,
) -> Result<Option<(Vec<u8>, String)>, AppError> {
    let path = cache.join(name);
    let tmp = cache.join(tmp_name(name));
    let main = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let Some(want) = want else {
        return Ok(main.map(|b| {
            let h = omr_store::jcs::sha384_hex(&b);
            (b, h)
        }));
    };
    if let Some(b) = main {
        let h = omr_store::jcs::sha384_hex(&b);
        if h == want {
            return Ok(Some((b, h)));
        }
    }
    match std::fs::read(&tmp) {
        Ok(b) if omr_store::jcs::sha384_hex(&b) == want => {
            if recover {
                std::fs::rename(&tmp, &path)?;
                sync_dir(cache);
            }
            Ok(Some((b, want.to_string())))
        }
        _ if !path.exists() => Err(crate::audit::damaged(&format!(
            "cache/{name} is missing although the audit log records it"
        ))),
        _ => Err(crate::audit::tampered(&format!(
            "cache/{name} is not the file the app saved"
        ))),
    }
}

/// Flushes a folder's entries (renames) to disk; best effort where folders cannot be opened.
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Writes `bytes` to `dir/name` durably: a private temporary file, synced, then renamed.
fn write_durable(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), AppError> {
    stage(dir, name, bytes)?;
    std::fs::rename(dir.join(tmp_name(name)), dir.join(name))?;
    sync_dir(dir);
    Ok(())
}

/// Writes `bytes` to the private temporary file of `dir/name`, synced (renamed later).
fn stage(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), AppError> {
    let tmp = dir.join(tmp_name(name));
    let _ = std::fs::remove_file(&tmp);
    crate::keys::write_private_new(&tmp, bytes)?;
    Ok(())
}

/// The refusal for a folder that holds no graded exam (R10: a failed grading run is named as
/// such, and the remedy differs).
fn no_exam(project_dir: &Path) -> AppError {
    let cache = project_dir.join("cache").join(SESSION_FILE);
    if project_dir.join("project.omr").exists() && !cache.exists() {
        AppError::NotFound(format!(
            "{}: a grading run started here but did not finish, so there is no graded exam; \
             delete the folder {} and grade the scans again",
            project_dir.display(),
            project_dir.display()
        ))
    } else {
        AppError::NotFound(format!(
            "{}: no graded exam here; run `omr grade … --out DIR` first, then point --project \
             at DIR",
            project_dir.display()
        ))
    }
}

impl std::fmt::Debug for ExamProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExamProject")
            .field("exam", &self.session.definition.title)
            .field("booklets", &self.session.booklets.len())
            .finish_non_exhaustive()
    }
}

fn reason_for(code: &str) -> &'static str {
    match code {
        "MRK-003" => "ERASURE_RESIDUE",
        "CAP-006" => "SCANNER_ARTIFACT",
        "REG-003" | "REG-004" | "CAP-009" => "TEMPLATE_MISALIGN",
        _ => "AMBIGUOUS_MARK",
    }
}

impl ExamProject {
    /// Grades inputs (see [`crate::pipeline::grade`]) and saves the session snapshot. A run
    /// that fails or is cancelled ([`crate::Cancel`]) leaves no half-made project behind (the
    /// package it created is removed, and the exam folder it created when nothing else is in
    /// it), so the same `--out` folder can be used again.
    pub fn grade(
        def: &ExamDefinition,
        inputs: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<Self, AppError> {
        Self::grade_fresh(opts, || crate::pipeline::grade(def, inputs, opts, progress))
    }

    /// Runs `run` into the package `opts.project_dir` names and saves the graded exam; on
    /// failure, removes what this call created (the package, and its folder when empty).
    fn grade_fresh(
        opts: &GradeOptions,
        run: impl FnOnce() -> Result<Graded, AppError>,
    ) -> Result<Self, AppError> {
        let fresh = opts.project_dir.as_ref().filter(|d| !d.exists()).cloned();
        // The exam folder that will hold the package and its exports is private to the user
        // when the app creates it (Restricted data, §19.3).
        let parent = fresh
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| !d.as_os_str().is_empty() && !d.exists())
            .map(Path::to_path_buf);
        if let Some(parent) = &parent {
            crate::keys::create_private_dir(parent)?;
        }
        let result = run().and_then(|g| {
            let mut p = Self::from_graded(g);
            p.save()?;
            p.judge_flags()?;
            Ok(p)
        });
        if result.is_err() {
            if let Some(dir) = fresh {
                let _ = std::fs::remove_dir_all(dir);
            }
            // Only an empty folder goes (nothing else was put in it meanwhile).
            if let Some(parent) = parent {
                let _ = std::fs::remove_dir(parent);
            }
        }
        result
    }

    /// [`Self::grade`] with key-sheet files (§10.1, `omr grade --key-sheet`): every sheet in
    /// `key_sheets` is a key sheet, read by the same recognizer and held for the instructor's
    /// confirmation ([`Self::confirm_key_sheets`]); nothing is scored from it until then.
    pub fn grade_with_key_sheets(
        def: &ExamDefinition,
        inputs: &[PathBuf],
        key_sheets: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<Self, AppError> {
        if key_sheets.is_empty() {
            return Self::grade(def, inputs, opts, progress);
        }
        Self::grade_fresh(opts, || {
            crate::pipeline::grade_with_key_sheets(def, inputs, key_sheets, opts, progress)
        })
    }

    /// Makes the originals of an exam graded in memory available for page images and crops
    /// (no project store holds them), matched by SHA-384.
    pub(crate) fn prefill_sources(&self, files: Vec<SourceFile>) {
        let mut cache = self.sources.lock().expect("sources lock");
        for f in files {
            if let Some(i) = self
                .session
                .sources
                .iter()
                .position(|s| s.sha384 == f.sha384)
            {
                cache.insert(i, Arc::new(f));
            }
        }
    }

    /// Wraps a pipeline result.
    pub fn from_graded(g: Graded) -> Self {
        Self {
            session: g.session,
            scoring: g.scoring,
            store: g.store,
            bound: None,
            dirty: Dirty {
                all: true,
                ..Dirty::default()
            },
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
            stability: Mutex::new(None),
        }
    }

    /// Reads the session snapshot (`cache/session.json`, the booklet chunks listed in
    /// `cache/booklets.json`, `cache/pages.json`) and checks every file against its binding
    /// in the chain (SHA-384) and the whole against its own indices.
    fn read_session(
        project_dir: &Path,
        binding: Option<&SnapshotBinding>,
        recover: bool,
    ) -> Result<(Session, Bound), AppError> {
        let cache = project_dir.join("cache");
        let Some((text, sh)) = read_bound(
            &cache,
            SESSION_FILE,
            binding.map(|b| b.session_sha384.as_str()),
            recover,
        )?
        else {
            return Err(no_exam(project_dir));
        };
        let Some((pages, ph)) = read_bound(
            &cache,
            PAGES_FILE,
            binding.map(|b| b.pages_sha384.as_str()),
            recover,
        )?
        else {
            return Err(crate::audit::damaged("cache/pages.json is missing"));
        };
        let mut session: Session = serde_json::from_slice(&text)
            .map_err(|e| crate::audit::damaged(&format!("cache/session.json: {e}")))?;
        session.pages = serde_json::from_slice(&pages)
            .map_err(|e| crate::audit::damaged(&format!("cache/pages.json: {e}")))?;
        let mut bound = Bound {
            session: sh,
            pages: ph,
            ..Bound::default()
        };
        // The booklets, in chunks (a snapshot written before chunks holds them in its header).
        if let Some(want) = binding.and_then(|b| b.index_sha384.as_deref()) {
            let Some((index, ih)) = read_bound(&cache, BOOKLET_INDEX_FILE, Some(want), recover)?
            else {
                return Err(crate::audit::damaged("cache/booklets.json is missing"));
            };
            let chunks: Vec<String> = serde_json::from_slice(&index)
                .map_err(|e| crate::audit::damaged(&format!("cache/booklets.json: {e}")))?;
            let mut booklets = Vec::new();
            for (c, h) in chunks.iter().enumerate() {
                let name = chunk_name(c);
                let Some((bytes, _)) = read_bound(&cache, &name, Some(h), recover)? else {
                    return Err(crate::audit::damaged(&format!("cache/{name} is missing")));
                };
                let part: Vec<crate::model::Booklet> = serde_json::from_slice(&bytes)
                    .map_err(|e| crate::audit::damaged(&format!("cache/{name}: {e}")))?;
                booklets.extend(part);
            }
            session.booklets = booklets;
            bound.index = ih;
            bound.chunks = chunks;
        }
        // The page-code secret of a generated sheet lives apart from the snapshot (a snapshot
        // written before that keeps its own copy).
        if let Some(want) = binding.and_then(|b| b.secret_sha384.as_deref()) {
            let path = project_dir.join(SECRETS_DIR).join(PAGE_CODE_KEY_FILE);
            let key = std::fs::read(&path).map_err(|_| {
                crate::audit::damaged(&format!(
                    "{SECRETS_DIR}/{PAGE_CODE_KEY_FILE} (the page-code secret) is missing"
                ))
            })?;
            if omr_store::jcs::sha384_hex(&key) != want {
                return Err(crate::audit::tampered(&format!(
                    "{SECRETS_DIR}/{PAGE_CODE_KEY_FILE} is not the page-code secret the exam was \
                     graded with"
                )));
            }
            session.definition.page_code_key_hex =
                Some(String::from_utf8_lossy(&key).trim().to_string());
        }
        crate::audit::validate(&session)?;
        Ok((session, bound))
    }

    /// Reopens a graded exam from its project package: the store (location, lock, chain,
    /// decision projections, this device's checkpoint), then the snapshot bound to it and
    /// checked against the store's decisions (SYS-011 when it was edited behind the app's
    /// back).
    pub fn open(project_dir: &Path, operator: &str) -> Result<Self, AppError> {
        if !project_dir.join("project.omr").exists() {
            return Err(no_exam(project_dir));
        }
        let store = match StoreWriter::open(project_dir, operator) {
            Ok(s) => s,
            // A grading run that stopped before its first snapshot: name it as such.
            Err(_) if !project_dir.join("cache").join(SESSION_FILE).exists() => {
                return Err(no_exam(project_dir));
            }
            Err(e) => return Err(e),
        };
        let binding = store.snapshot_binding()?;
        let (session, bound) = Self::read_session(project_dir, binding.as_ref(), true)?;
        crate::audit::against_store(
            &session,
            &store.reader(),
            store.exam(),
            Some(store.recog_run()),
            binding.as_ref().map(|b| b.seq),
        )?;
        let scoring = Scoring::build(&session)?;
        Ok(Self {
            session,
            scoring,
            store: Some(store),
            bound: Some(bound),
            dirty: Dirty::default(),
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
            stability: Mutex::new(None),
        })
    }

    /// Opens a graded exam read-only (no store lock): dashboards and previews (crops need the
    /// store, so they are unavailable). The same checks as [`Self::open`] run on a read-only
    /// connection.
    pub fn open_read_only(project_dir: &Path) -> Result<Self, AppError> {
        if !project_dir.join("project.omr").exists() {
            return Err(no_exam(project_dir));
        }
        let ro = crate::store::open_read_only(project_dir)?;
        let reader = ro.reader();
        let binding = crate::store::snapshot_binding(&reader)?;
        let (session, bound) = Self::read_session(project_dir, binding.as_ref(), false)?;
        let exam = ro
            .header()
            .assessment_id
            .as_deref()
            .and_then(|a| uuid::Uuid::parse_str(a).ok())
            .map(omr_model::ExamId)
            .ok_or_else(|| crate::audit::damaged("the package names no exam"))?;
        let run = reader
            .last_event("RecognitionRunStarted")
            .map_err(|e| AppError::Store(e.to_string()))?
            .map(|e| e.entity_id);
        crate::audit::against_store(
            &session,
            &reader,
            exam,
            run.as_deref(),
            binding.as_ref().map(|b| b.seq),
        )?;
        let scoring = Scoring::build(&session)?;
        Ok(Self {
            session,
            scoring,
            store: None,
            bound: Some(bound),
            dirty: Dirty::default(),
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
            stability: Mutex::new(None),
        })
    }

    /// The absent list (§9.6 batch close, §10.5): roster students no booklet names as its
    /// student, in roster order. Empty without a roster (nobody is known to be absent). While
    /// an identity item is open the list is provisional, and release is blocked by that item
    /// (§13.1: nobody is released as absent while identity evidence is open).
    pub fn absent_students(&self) -> Vec<&crate::exam::RosterStudent> {
        let Some(roster) = &self.session.definition.roster else {
            return Vec::new();
        };
        let seen: std::collections::BTreeSet<String> = self
            .session
            .booklets
            .iter()
            .filter_map(|b| b.student().map(omr_identity::normalize_id))
            .collect();
        roster
            .iter()
            .filter(|r| !seen.contains(&omr_identity::normalize_id(&r.id)))
            .collect()
    }

    /// Sets the acting principal for subsequent review events.
    pub fn set_actor(&mut self, id: &str, role: &str) {
        if let Some(s) = self.store.as_mut() {
            s.set_actor(id, role);
        }
    }

    /// The project package root, if stored.
    pub fn root(&self) -> Option<PathBuf> {
        self.store.as_ref().map(StoreWriter::root)
    }

    /// The store (events, objects).
    pub fn store(&self) -> Option<&StoreWriter> {
        self.store.as_ref()
    }

    /// Saves the session snapshot into the package's `cache/` and binds it to the chain
    /// (§14.4). The snapshot is the session header (`session.json`), the booklets in chunks of
    /// [`BOOKLET_CHUNK`] (`booklets-NNNNN.json`, listed with their SHA-384 in
    /// `booklets.json`) and the pages (`pages.json`, written once). Only what changed is
    /// rewritten: a review action costs the header and one chunk, not the whole exam (CR-14).
    /// Every new file is written to a private temporary file and synced, a `SnapshotSaved`
    /// event records the SHA-384 of the header, the chunk index, the pages and the page-code
    /// secret, then the files are renamed into place and the folder synced. An interrupted save
    /// is completed on the next open; a snapshot edited outside the app no longer opens
    /// (SYS-011). Nothing is written when nothing changed.
    pub fn save(&mut self) -> Result<(), AppError> {
        // The release gate's mis-key check follows the score run (§12.2): a new run reads
        // "computing…" until `judge_flags` has run for it.
        self.session.flags.current_run = self.current_run_id();
        let Some(st) = self.store.as_mut() else {
            return Ok(());
        };
        let sha = omr_store::jcs::sha384_hex;
        let dir = st.cache_dir();
        crate::keys::create_private_dir(&dir)?;
        let pages_path = dir.join(PAGES_FILE);
        let pages_sha = match (&self.bound, pages_path.exists()) {
            (Some(b), true) if !b.pages.is_empty() => b.pages.clone(),
            (_, true) => sha(&std::fs::read(&pages_path)?),
            (_, false) => {
                let bytes = serde_json::to_vec(&self.session.pages)
                    .map_err(|e| AppError::Internal(format!("page snapshot: {e}")))?;
                write_durable(&dir, PAGES_FILE, &bytes)?;
                sha(&bytes)
            }
        };
        // The page-code secret goes to the package's private folder, never into the snapshot.
        let secret = match &self.session.definition.page_code_key_hex {
            Some(k) => {
                let sdir = st.root().join(SECRETS_DIR);
                let path = sdir.join(PAGE_CODE_KEY_FILE);
                if !path.exists() {
                    crate::keys::create_private_dir(&sdir)?;
                    write_durable(&sdir, PAGE_CODE_KEY_FILE, k.as_bytes())?;
                }
                Some(sha(&std::fs::read(&path)?))
            }
            None => None,
        };
        // The booklet chunks that changed.
        let booklets = &self.session.booklets;
        let n_chunks = booklets.len().div_ceil(BOOKLET_CHUNK);
        let mut chunks = self
            .bound
            .as_ref()
            .map(|b| b.chunks.clone())
            .filter(|c| c.len() == n_chunks && !self.dirty.all)
            .unwrap_or_default();
        let rewrite: std::collections::BTreeSet<usize> = if chunks.len() != n_chunks {
            chunks = vec![String::new(); n_chunks];
            (0..n_chunks).collect()
        } else {
            self.dirty
                .booklets
                .iter()
                .map(|b| b / BOOKLET_CHUNK)
                .filter(|&c| c < n_chunks)
                .collect()
        };
        let chunk_bytes = |c: usize| {
            let end = ((c + 1) * BOOKLET_CHUNK).min(booklets.len());
            serde_json::to_vec(&booklets[c * BOOKLET_CHUNK..end])
                .map_err(|e| AppError::Internal(format!("booklet snapshot: {e}")))
        };
        let mut staged: Vec<String> = Vec::new();
        for c in rewrite {
            let bytes = chunk_bytes(c)?;
            let h = sha(&bytes);
            if h != chunks[c] || !dir.join(chunk_name(c)).exists() {
                stage(&dir, &chunk_name(c), &bytes)?;
                staged.push(chunk_name(c));
                chunks[c] = h;
            }
        }
        // Every booklet change was marked dirty (checked in debug builds: a missed mark would
        // leave a stale chunk that the next open refuses as tampering).
        #[cfg(debug_assertions)]
        for (c, h) in chunks.iter().enumerate() {
            debug_assert_eq!(
                sha(&chunk_bytes(c)?),
                *h,
                "booklet chunk {c} changed without being marked dirty"
            );
        }
        let index = serde_json::to_vec(&chunks)
            .map_err(|e| AppError::Internal(format!("booklet index: {e}")))?;
        let header = serde_json::to_vec(&self.session)
            .map_err(|e| AppError::Internal(format!("session snapshot: {e}")))?;
        let now = Bound {
            session: sha(&header),
            pages: pages_sha,
            index: sha(&index),
            chunks,
        };
        let on_disk = [SESSION_FILE, BOOKLET_INDEX_FILE]
            .iter()
            .all(|f| dir.join(f).exists());
        if self.bound.as_ref() == Some(&now) && on_disk {
            self.dirty = Dirty::default();
            return Ok(());
        }
        let prev = self.bound.clone().unwrap_or_default();
        if prev.index != now.index || !dir.join(BOOKLET_INDEX_FILE).exists() {
            stage(&dir, BOOKLET_INDEX_FILE, &index)?;
            staged.push(BOOKLET_INDEX_FILE.into());
        }
        if prev.session != now.session || !dir.join(SESSION_FILE).exists() {
            stage(&dir, SESSION_FILE, &header)?;
            staged.push(SESSION_FILE.into());
        }
        st.record_snapshot(&now.session, &now.pages, &now.index, secret.as_deref())?;
        for name in &staged {
            std::fs::rename(dir.join(tmp_name(name)), dir.join(name))?;
        }
        sync_dir(&dir);
        // Chunks past the end (fewer booklets after a regrade) are no longer listed.
        let mut c = n_chunks;
        while std::fs::remove_file(dir.join(chunk_name(c))).is_ok() {
            c += 1;
        }
        self.bound = Some(now);
        self.dirty = Dirty::default();
        Ok(())
    }

    /// Marks booklet `bi` changed (its snapshot chunk is rewritten at the next save). The
    /// exam's own actions do this; a caller that changes `session.booklets` directly must.
    pub fn touch(&mut self, bi: usize) {
        self.dirty.booklets.insert(bi);
    }

    /// Marks every booklet changed.
    fn touch_all(&mut self) {
        self.dirty.all = true;
    }

    /// Closes the store (snapshot saved, signed checkpoint written).
    pub fn close(mut self) -> Result<(), AppError> {
        self.save()?;
        match self.store {
            Some(s) => s.close(),
            None => Ok(()),
        }
    }

    /// Open items of a lane (all lanes for `None`) in queue order.
    pub fn queue(&self, lane: Option<Lane>) -> Vec<&ReviewItem> {
        self.session.review.queue(lane)
    }

    fn next_after(&self, lane: Lane, current: u64) -> Option<u64> {
        let q = self.session.review.queue(Some(lane));
        let pos = q.iter().position(|i| i.id == current);
        match pos {
            Some(p) => q.get(p + 1).or_else(|| q.first()).map(|i| i.id),
            None => q.first().map(|i| i.id),
        }
        .filter(|&id| id != current)
    }

    /// Applies a review action (§13.3): records the audited event(s), updates the booklet and
    /// rescores incrementally. Without a reason, a decision that overrides the engine's
    /// proposal is refused (§13.4): use [`Self::apply_with_reason`].
    pub fn apply(&mut self, item_id: u64, action: Action) -> Result<ApplyOutcome, AppError> {
        self.apply_with_reason(item_id, action, None)
    }

    /// True when `decision` on `item` overrides the engine's proposal (§13.4): a Mark,
    /// Identity or Form decision other than the suggestion, when the engine made one. A
    /// rescan request (unreadable) and acknowledgements are not overrides.
    pub fn overrides_engine(&self, item_id: u64, decision: &Decision) -> bool {
        let Some(item) = self.session.review.item(item_id) else {
            return false;
        };
        // A probable mis-key (SCO-001) is never kept without the instructor's reason (§12.2:
        // "keep with a note"); a key change carries KEY_ERROR by default.
        if item.flag.is_some() {
            return matches!(decision, Decision::Acknowledge);
        }
        let Some(sugg) = &item.suggestion else {
            return false;
        };
        let d = match (item.kind, decision) {
            (_, Decision::Acknowledge | Decision::Unreadable) => return false,
            (ItemKind::Notice, _) => return false,
            // The engine assigned nobody (not on the roster): confirming that is no override.
            (ItemKind::Identity, Decision::NotOnRoster)
                if item
                    .booklet
                    .is_some_and(|b| self.session.booklets[b].identity.student.is_none()) =>
            {
                return false;
            }
            (ItemKind::Identity, Decision::Option(n)) => match item.booklet.and_then(|b| {
                self.session.booklets[b]
                    .identity
                    .candidates
                    .get(usize::from(*n))
            }) {
                Some(c) => Decision::Student(c.0.clone()),
                None => return false,
            },
            (ItemKind::Identity, Decision::Student(s)) => {
                Decision::Student(omr_identity::normalize_id(s))
            }
            (ItemKind::Form, Decision::Option(f)) => Decision::Form(*f),
            // "Multi" without options takes the proposal's marks (the shell's `M` key).
            (ItemKind::Mark, Decision::Multi(v)) if v.is_empty() => {
                if matches!(sugg, Decision::Multi(_)) {
                    return false;
                }
                decision.clone()
            }
            (_, d) => d.clone(),
        };
        let s = match sugg {
            Decision::Student(x) => Decision::Student(omr_identity::normalize_id(x)),
            other => other.clone(),
        };
        d != s
    }

    /// [`Self::apply`] with the reviewer's reason (§13.4, §14.4): recorded on every audit event
    /// of the action (the note as a hash reference only). The reason is required when the
    /// decision overrides the engine's proposal; `OTHER` and `SOLO_SELF_APPROVAL` need a note.
    /// Without one, a confirmation records the item's default code (e.g. `AMBIGUOUS_MARK`).
    pub fn apply_with_reason(
        &mut self,
        item_id: u64,
        action: Action,
        reason: Option<ReviewReason>,
    ) -> Result<ApplyOutcome, AppError> {
        if let Some(r) = &reason {
            r.validate().map_err(AppError::Review)?;
        }
        let decision = match &action {
            Action::Resolve { decision, .. } | Action::ApplyToGroup { decision } => Some(decision),
            _ => None,
        };
        if reason.is_none()
            && let Some(d) = decision
            && self.overrides_engine(item_id, d)
        {
            let it = self
                .session
                .review
                .item(item_id)
                .expect("checked by overrides_engine");
            let def = &self.session.definition;
            let codes = crate::review::reasons_for(it.kind, it.lane);
            return Err(AppError::Review(format!(
                "{} overrides the engine's proposal ({}): give a reason code (§13.4), e.g. {} \
                 (CLI: --reason CODE [--note TEXT])",
                d.text(&def.option_labels(), &def.forms),
                it.suggestion
                    .as_ref()
                    .map(|s| s.text(&def.option_labels(), &def.forms))
                    .unwrap_or_default(),
                codes.join(", ")
            )));
        }
        let item = self
            .session
            .review
            .item(item_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("review item {item_id}")))?;
        let lane = item.lane;
        // A resolved item stays resolved; only `undo` re-opens it (with its booklet data and
        // the store's exception rows).
        if !item.is_open() {
            return Err(AppError::Review(format!(
                "item {item_id} is already resolved (use undo to re-open it)"
            )));
        }
        if item.flag.is_some() {
            if matches!(action, Action::ApplyToGroup { .. }) {
                return Err(AppError::Review(format!(
                    "item {item_id} is a probable mis-key (SCO-001): decide it on its own"
                )));
            }
            if let Action::Resolve {
                decision: Decision::Acknowledge,
                ..
            } = &action
                && reason.as_ref().is_none_or(|r| r.note.trim().is_empty())
            {
                return Err(AppError::Review(format!(
                    "keeping the key of item {item_id}, a probable mis-key (SCO-001), needs a \
                     reason code and a note saying why (§12.2), e.g. --reason ADJUDICATION --note \
                     \"checked with the instructor: the key is right\""
                )));
            }
        }
        let mut changes = Vec::new();
        // A group member that refuses the decision after others took it (e.g. one student
        // given to two booklets, IDN-005): the applied part is recorded and then undone, so
        // the exam and its audit trail are as before the action.
        let mut refused: Option<(u64, AppError)> = None;
        // The reviewer's reason in its store form, once per action (a note is stored once).
        let given = match &reason {
            Some(r) => Some(self.stored_reason(r)?),
            None => None,
        };
        match &action {
            Action::Resolve { decision, .. } => {
                changes.push(self.resolve_one(&item, decision, given.as_ref())?);
            }
            Action::ApplyToGroup { decision } => {
                let members: Vec<ReviewItem> = self
                    .session
                    .review
                    .group_members(&item)
                    .into_iter()
                    .cloned()
                    .collect();
                for m in &members {
                    if m.severity == Severity::Blocker {
                        return Err(AppError::Review(format!(
                            "item {} is a BLOCKER: batch confirm refused (§13.2)",
                            m.id
                        )));
                    }
                    if m.crop.as_ref().is_some_and(|c| {
                        c.bubbles
                            .iter()
                            .any(|b| b.state == crate::review::OverlayState::Unreadable)
                    }) {
                        return Err(AppError::Review(format!(
                            "item {} has an UNREADABLE bubble: batch confirm refused (§13.2)",
                            m.id
                        )));
                    }
                    // §10.3: forms are confirmed as a group only where the answers single the
                    // version out by 10 nats, and never for a sheet bubbling a version the
                    // exam does not declare.
                    if m.kind == ItemKind::Form {
                        if m.sub_reason.as_deref() == Some(crate::infer::UNDECLARED_VERSION) {
                            return Err(AppError::Review(format!(
                                "item {} is a sheet bubbling a version this exam does not \
                                 declare: decide its version on its own (§10.3)",
                                m.id
                            )));
                        }
                        let confident = m
                            .booklet
                            .and_then(|b| self.session.booklets[b].inference.as_ref())
                            .is_some_and(crate::infer::confident);
                        if !confident {
                            return Err(AppError::Review(format!(
                                "item {}: the answers fit its suggested version by less than \
                                 {} nats, so it is not confirmed as a group; decide it on its \
                                 own (§10.3)",
                                m.id,
                                crate::infer::DELTA_LL_NATS
                            )));
                        }
                    }
                }
                for m in &members {
                    match self.resolve_one(m, decision, given.as_ref()) {
                        Ok(c) => changes.push(c),
                        Err(e) if !changes.is_empty() => {
                            refused = Some((m.id, e));
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            Action::Defer | Action::Escalate | Action::AddToRescan => {
                let it = self.session.review.item_mut(item_id).expect("item exists");
                changes.push(Change {
                    item: item_id,
                    state_before: it.state,
                    resolution_before: it.resolution.clone(),
                    deferrals_before: it.deferrals,
                    response_before: None,
                    identity_before: None,
                    form_before: None,
                    hold_before: None,
                    resolved_rows: Vec::new(),
                });
                let (state, ty) = match action {
                    Action::Defer => (ItemState::Deferred, "ReviewItemDeferred"),
                    Action::Escalate => (ItemState::Escalated, "ReviewItemEscalated"),
                    _ => (ItemState::Rescan, "RescanRequested"),
                };
                it.state = state;
                if state == ItemState::Deferred {
                    it.deferrals += 1;
                }
                if let Some(st) = self.store.as_mut() {
                    match &given {
                        Some(r) => st.event_with_reason(
                            ty,
                            "review_item",
                            &item_id.to_string(),
                            json!({"code": item.code}),
                            r,
                        )?,
                        None => st.event(
                            ty,
                            "review_item",
                            &item_id.to_string(),
                            json!({"code": item.code}),
                        )?,
                    }
                }
            }
        }
        let affected: Vec<u64> = changes.iter().map(|c| c.item).collect();
        let seq = self.session.review.history.len() as u64 + 1;
        let actor = self
            .store
            .as_ref()
            .map_or_else(|| "principal:local".to_string(), |s| s.actor().id.clone());
        self.session.review.history.push(ReviewRecord {
            seq,
            item: item_id,
            action,
            changes,
            actor,
            undone: false,
            reason,
        });
        if let Some((id, e)) = refused {
            self.undo()?;
            return Err(AppError::Review(format!(
                "item {id} of the group refused the decision ({e}); the items before it were \
                 reverted, so nothing changed: resolve the items one by one"
            )));
        }
        for id in &affected {
            if let Some(b) = self.session.review.item(*id).and_then(|i| i.booklet) {
                self.touch(b);
            }
        }
        self.after_release("review decision")?;
        self.save()?;
        Ok(ApplyOutcome {
            next: self.next_after(lane, item_id),
            affected,
        })
    }

    /// A reviewer's reason in its store form (a note kept in the store under a salted digest,
    /// SEC-08); without a store, the in-memory form.
    fn stored_reason(&mut self, r: &ReviewReason) -> Result<omr_store::Reason, AppError> {
        match self.store.as_mut() {
            Some(st) => st.reason(&r.code, &r.note),
            None => Ok(store_reason(&r.code, &r.note)),
        }
    }

    fn resolve_one(
        &mut self,
        item: &ReviewItem,
        decision: &Decision,
        given: Option<&omr_store::Reason>,
    ) -> Result<Change, AppError> {
        // The reviewer's reason, else the item's default code (§14.4).
        let why = |default: &str| match given {
            Some(r) => r.clone(),
            None => store_reason(default, ""),
        };
        let def = &self.session.definition;
        let mut change = Change {
            item: item.id,
            state_before: item.state,
            resolution_before: item.resolution.clone(),
            deferrals_before: item.deferrals,
            response_before: None,
            identity_before: None,
            form_before: None,
            hold_before: None,
            resolved_rows: Vec::new(),
        };
        let decision = match decision {
            Decision::Acknowledge => item.suggestion.clone().unwrap_or(Decision::Acknowledge),
            d => d.clone(),
        };
        let mut new_state = ItemState::Resolved;
        match item.kind {
            ItemKind::Mark => {
                let (bi, q) = (
                    item.booklet
                        .ok_or_else(|| AppError::Internal("mark item without booklet".into()))?,
                    usize::from(item.question.unwrap_or(0)),
                );
                let new = match &decision {
                    Decision::Option(o) if *o < def.options => Response::Single(*o),
                    Decision::Blank => Response::Blank,
                    Decision::Multi(v) if v.len() >= 2 && v.iter().all(|&o| o < def.options) => {
                        Response::Multi(v.clone())
                    }
                    // "Multi" without options (the shell's key `M`): the proposal's marks, or
                    // every option whose fill reaches the review band.
                    Decision::Multi(v) if v.is_empty() => {
                        let bi = item.booklet.unwrap_or(0);
                        let q = usize::from(item.question.unwrap_or(0));
                        let marked: Vec<u8> =
                            match &self.session.booklets[bi].questions[q].recognized.response {
                                Response::Multi(m) => m.clone(),
                                _ => {
                                    let lo = item.crop.as_ref().map_or(0.08, |c| c.band.0);
                                    item.fills
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, f)| **f >= lo)
                                        .map(|(o, _)| o as u8)
                                        .collect()
                                }
                            };
                        if marked.len() < 2 {
                            return Err(AppError::Review(
                                "fewer than two marked options: choose the options of the multi"
                                    .into(),
                            ));
                        }
                        Response::Multi(marked)
                    }
                    Decision::Unreadable => {
                        new_state = ItemState::Rescan;
                        Response::Unreadable { bubbles: vec![] }
                    }
                    other => {
                        return Err(AppError::Review(format!(
                            "{} is not a valid answer for a question with {} options",
                            other.text(&def.option_labels(), &def.forms),
                            def.options
                        )));
                    }
                };
                let b = &mut self.session.booklets[bi];
                let qs = &mut b.questions[q];
                change.response_before = Some((qs.current.clone(), qs.status));
                let old = qs.current.clone();
                if new_state == ItemState::Resolved {
                    qs.status = if new == qs.recognized.response {
                        ResponseStatus::Confirmed
                    } else {
                        ResponseStatus::Changed
                    };
                    qs.current = new.clone();
                }
                let booklet_id = b.booklet_id.clone();
                let review_seq = self.session.review.history.len() + 1;
                if let Some(st) = self.store.as_mut() {
                    if new_state == ItemState::Resolved {
                        st.correction(
                            &booklet_id,
                            q as u16,
                            &old,
                            &new,
                            &why(reason_for(&item.code)),
                            &item.store_exceptions,
                        )?;
                        change.resolved_rows = item.store_exceptions.clone();
                        // A parallel-run difference decided: a field label (§23.7, §23.2).
                        if let Some(inc) = &item.incumbent {
                            let labels = def.option_labels();
                            let decided = crate::review::suggestion_of(&new);
                            let text = |d: &Option<Decision>| {
                                d.as_ref().map(|d| d.text(&labels, &def.forms))
                            };
                            st.event(
                                "FieldLabelRecorded",
                                "booklet",
                                &booklet_id,
                                json!({
                                    "position": q,
                                    "source": "parallel_run",
                                    "incumbent": inc.system,
                                    "engine_reading": inc.engine.text(&labels, &def.forms),
                                    "incumbent_reading": inc.text,
                                    "adjudicated": text(&decided),
                                    "engine_loss": decided.as_ref() != Some(&inc.engine),
                                    "incumbent_loss": decided.is_none() || inc.decision != decided,
                                    "review_seq": review_seq,
                                }),
                            )?;
                        }
                    } else {
                        st.event(
                            "RescanRequested",
                            "booklet",
                            &booklet_id,
                            json!({"position": q, "code": item.code}),
                        )?;
                    }
                }
                if let Some(s) = self.scoring.as_mut() {
                    s.update_booklet(bi, &self.session.booklets[bi])?;
                }
            }
            ItemKind::Identity => {
                let bi = item
                    .booklet
                    .ok_or_else(|| AppError::Internal("identity item without booklet".into()))?;
                let b = &mut self.session.booklets[bi];
                let student = match &decision {
                    Decision::Student(s) => Some(omr_identity::normalize_id(s)),
                    Decision::Option(n) => Some(
                        b.identity
                            .candidates
                            .get(usize::from(*n))
                            .map(|c| c.0.clone())
                            .ok_or_else(|| AppError::Review(format!("no candidate {}", n + 1)))?,
                    ),
                    Decision::NotOnRoster => None,
                    other => {
                        return Err(AppError::Review(format!(
                            "{} is not an identity decision",
                            other.text(&def.option_labels(), &def.forms)
                        )));
                    }
                };
                if let (Some(s), Some(roster)) = (&student, &def.roster)
                    && !roster
                        .iter()
                        .any(|r| omr_identity::normalize_id(&r.id) == *s)
                {
                    return Err(AppError::Review(format!(
                        "{s} is not on the roster; choose N (not on roster) instead"
                    )));
                }
                // Two booklets never resolve to one student by a review decision (IDN-005): a
                // student already held by another booklet whose identity is settled (no open
                // identity item) is refused.
                if let Some(s) = &student
                    && let Some(other) = self.session.booklets.iter().enumerate().find(|(oi, o)| {
                        *oi != bi
                            && o.identity.student.as_deref() == Some(s.as_str())
                            && !self.session.review.items.iter().any(|it| {
                                it.kind == ItemKind::Identity
                                    && it.booklet == Some(*oi)
                                    && it.is_open()
                            })
                    })
                {
                    return Err(AppError::Review(format!(
                        "{s} is already the student of {} (IDN-005); choose another student or \
                         N (not on roster)",
                        other.1.label()
                    )));
                }
                let b = &mut self.session.booklets[bi];
                change.identity_before = Some((b.identity.student.clone(), b.identity.confirmed));
                b.identity.student = student.clone();
                b.identity.confirmed = true;
                let booklet_id = b.booklet_id.clone();
                if let Some(st) = self.store.as_mut() {
                    let r = why(if student.is_some() {
                        "ID_CORRECTION"
                    } else {
                        "NOT_ON_ROSTER"
                    });
                    st.confirm_identity(&booklet_id, student.as_deref(), &r)?;
                    st.resolve(&item.store_exceptions, &r, "student")?;
                    change.resolved_rows = item.store_exceptions.clone();
                }
                if let Some(s) = self.scoring.as_mut() {
                    s.update_booklet(bi, &self.session.booklets[bi])?;
                }
            }
            ItemKind::Form => {
                let bi = item
                    .booklet
                    .ok_or_else(|| AppError::Internal("form item without booklet".into()))?;
                let f = match &decision {
                    Decision::Form(f) | Decision::Option(f)
                        if usize::from(*f) < def.forms.len() =>
                    {
                        usize::from(*f)
                    }
                    other => {
                        return Err(AppError::Review(format!(
                            "{} is not a form of this exam",
                            other.text(&def.option_labels(), &def.forms)
                        )));
                    }
                };
                let b = &mut self.session.booklets[bi];
                change.form_before = Some((b.form, b.form_source));
                let was_scored = b.form.is_some();
                b.form = Some(f);
                b.form_source = FormSource::Reviewed;
                let booklet_id = b.booklet_id.clone();
                if let Some(st) = self.store.as_mut() {
                    let r = why("CLERICAL_CORRECTION");
                    st.event_with_reason(
                        "BookletFormConfirmed",
                        "booklet",
                        &booklet_id,
                        json!({"form": f as u64 + 1}),
                        &r,
                    )?;
                    st.resolve(&item.store_exceptions, &r, "form")?;
                    change.resolved_rows = item.store_exceptions.clone();
                }
                self.rescore_booklet(bi, was_scored)?;
            }
            ItemKind::Notice if item.flag.is_some() => {
                // A probable mis-key (§12.2): change the key to an option, accept the key or d,
                // void the item, or keep the key (with a reason and a note, checked above).
                let fl = item.flag.clone().expect("checked");
                let (form, q, d) = (fl.form.unwrap_or(0), usize::from(fl.question), fl.option);
                let labels = def.option_labels();
                let letter = |o: u8| {
                    labels
                        .get(usize::from(o))
                        .cloned()
                        .unwrap_or_else(|| (o + 1).to_string())
                };
                let cur = self
                    .session
                    .key
                    .as_ref()
                    .and_then(|k| k.forms.get(form))
                    .and_then(|f| f.get(q))
                    .cloned()
                    .ok_or_else(|| AppError::Review("the exam has no key entry for it".into()))?;
                let cell = match &decision {
                    Decision::Option(o) if *o < def.options => Some(KeyCell::Options(vec![*o])),
                    Decision::Multi(v) => {
                        let mut both = match &cur {
                            KeyCell::Options(k) => k.clone(),
                            _ => Vec::new(),
                        };
                        both.push(d);
                        both.sort_unstable();
                        both.dedup();
                        let mut given = v.clone();
                        given.sort_unstable();
                        given.dedup();
                        if !given.is_empty() && given != both {
                            return Err(AppError::Review(format!(
                                "accepting a second answer accepts the key or {}: decide `multi` \
                                 without options",
                                letter(d)
                            )));
                        }
                        Some(KeyCell::Options(both))
                    }
                    Decision::Void => Some(KeyCell::Void),
                    Decision::Acknowledge => None,
                    other => {
                        return Err(AppError::Review(format!(
                            "{} does not decide a probable mis-key: change the key to an option \
                             (a letter), accept the key or {} (multi), void the item (void), or \
                             keep the key with a reason and a note (ack)",
                            other.text(&labels, &def.forms),
                            letter(d)
                        )));
                    }
                };
                match cell {
                    Some(c) if c == cur => {
                        return Err(AppError::Review(
                            "that is the key in force: to keep it, acknowledge (ack) with a \
                             reason code and a note"
                                .into(),
                        ));
                    }
                    Some(c) => {
                        let r = why(if c == KeyCell::Void {
                            "ITEM_VOIDED_FLAWED"
                        } else {
                            "KEY_ERROR"
                        });
                        self.change_key_entry(form, q, c, &r)?;
                    }
                    None => {
                        if let Some(st) = self.store.as_mut() {
                            st.event_with_reason(
                                "ReviewItemAcknowledged",
                                "review_item",
                                &item.id.to_string(),
                                json!({"code": item.code, "sub_reason": item.sub_reason}),
                                &why("ADJUDICATION"),
                            )?;
                        }
                    }
                }
            }
            ItemKind::Notice => {
                if !matches!(decision, Decision::Acknowledge) {
                    return Err(AppError::Review(
                        "page- and exam-level items are acknowledged (A)".into(),
                    ));
                }
                if let Some(st) = self.store.as_mut() {
                    let r = why(if item.lane == Lane::Capture {
                        "NOT_EXAM_MATERIAL"
                    } else {
                        "ADJUDICATION"
                    });
                    if item.store_exceptions.is_empty() {
                        st.event_with_reason(
                            "ReviewItemAcknowledged",
                            "review_item",
                            &item.id.to_string(),
                            json!({"code": item.code}),
                            &r,
                        )?;
                    } else {
                        st.resolve(&item.store_exceptions, &r, "acknowledge")?;
                    }
                    change.resolved_rows = item.store_exceptions.clone();
                }
                // Acknowledging a blank booklet releases its held score (scored as marked).
                if let Some(bi) = item.booklet
                    && self.session.booklets[bi].hold.as_deref() == Some(item.code.as_str())
                {
                    change.hold_before = Some(self.session.booklets[bi].hold.take());
                    if let Some(s) = self.scoring.as_mut() {
                        s.update_booklet(bi, &self.session.booklets[bi])?;
                    }
                }
            }
        }
        let it = self.session.review.item_mut(item.id).expect("item exists");
        it.state = new_state;
        it.resolution = Some(decision);
        Ok(change)
    }

    fn rescore_booklet(&mut self, bi: usize, was_scored: bool) -> Result<(), AppError> {
        let in_matrix = self
            .scoring
            .as_ref()
            .is_some_and(|s| s.row_of.get(bi).copied().flatten().is_some());
        if was_scored && in_matrix {
            if let Some(s) = self.scoring.as_mut() {
                s.update_booklet(bi, &self.session.booklets[bi])?;
            }
        } else {
            self.scoring = Scoring::build(&self.session)?;
        }
        Ok(())
    }

    /// Compensates the latest not-undone review event; returns the item it re-opened.
    pub fn undo(&mut self) -> Result<Option<u64>, AppError> {
        let sealed = self.session.review.sealed;
        let Some(pos) = self.session.review.history.iter().rposition(|r| !r.undone) else {
            return Ok(None);
        };
        if pos < sealed {
            return Err(AppError::Review(
                "the latest review event belongs to the grading run before a batch was \
                 appended; its decision was carried over and can no longer be undone \
                 (resolve the item again instead)"
                    .into(),
            ));
        }
        let rec = self.session.review.history[pos].clone();
        for c in &rec.changes {
            if let Some(b) = self.session.review.item(c.item).and_then(|i| i.booklet) {
                self.touch(b);
            }
        }
        for c in rec.changes.iter().rev() {
            let item = self
                .session
                .review
                .item(c.item)
                .cloned()
                .ok_or_else(|| AppError::Internal("undo of an unknown item".into()))?;
            if let (Some((resp, status)), Some(bi)) = (&c.response_before, item.booklet) {
                let q = usize::from(item.question.unwrap_or(0));
                let b = &mut self.session.booklets[bi];
                let now = b.questions[q].current.clone();
                b.questions[q].current = resp.clone();
                b.questions[q].status = *status;
                let booklet_id = b.booklet_id.clone();
                if let Some(st) = self.store.as_mut()
                    && now != *resp
                {
                    st.correction(
                        &booklet_id,
                        q as u16,
                        &now,
                        resp,
                        &store_reason("CLERICAL_CORRECTION", ""),
                        &[],
                    )?;
                }
                if let Some(s) = self.scoring.as_mut() {
                    s.update_booklet(bi, &self.session.booklets[bi])?;
                }
            }
            if let (Some((student, confirmed)), Some(bi)) = (&c.identity_before, item.booklet) {
                let b = &mut self.session.booklets[bi];
                b.identity.student = student.clone();
                b.identity.confirmed = *confirmed;
                let (booklet_id, status) = (b.booklet_id.clone(), b.identity.status);
                if let Some(st) = self.store.as_mut() {
                    st.restore_identity(&booklet_id, student.as_deref(), status, *confirmed)?;
                }
                if let Some(s) = self.scoring.as_mut() {
                    s.update_booklet(bi, &self.session.booklets[bi])?;
                }
            }
            if let (Some(hold), Some(bi)) = (&c.hold_before, item.booklet) {
                self.session.booklets[bi].hold = hold.clone();
                if let Some(s) = self.scoring.as_mut() {
                    s.update_booklet(bi, &self.session.booklets[bi])?;
                }
            }
            if let (Some((form, src)), Some(bi)) = (&c.form_before, item.booklet) {
                let b = &mut self.session.booklets[bi];
                b.form = *form;
                b.form_source = *src;
                self.scoring = Scoring::build(&self.session)?;
            }
            if let Some(st) = self.store.as_mut() {
                st.reopen(&c.resolved_rows)?;
                st.event(
                    "ReviewActionUndone",
                    "review_item",
                    &c.item.to_string(),
                    json!({"seq": rec.seq}),
                )?;
            }
            let it = self.session.review.item_mut(c.item).expect("item exists");
            it.state = c.state_before;
            it.resolution = c.resolution_before.clone();
            it.deferrals = c.deferrals_before;
        }
        self.session.review.history[pos].undone = true;
        self.after_release("review decision undone")?;
        self.save()?;
        Ok(Some(rec.item))
    }

    /// Sets the key of one question of one form — `form` is the 0-based index into the
    /// definition's forms, `question` the **1-based** printed question (as in the mapping
    /// editor and the shell); `answer`: "", "B", "A|C" or "A,C", "*" — a new key version,
    /// rescored without re-projecting (L2, A4). With identical forms (shared master items)
    /// every form's copy of the key changes together.
    pub fn set_key(&mut self, form: usize, question: u32, answer: &str) -> Result<(), AppError> {
        self.set_key_with_reason(form, question, answer, None)
    }

    /// [`Self::set_key`] with the reason recorded on the key version (§10.8, §14.4); without
    /// one, `KEY_ERROR` when an entry changes and `CLERICAL_CORRECTION` when a missing entry
    /// is filled in. The new key version is published in the store (audited regrade).
    pub fn set_key_with_reason(
        &mut self,
        form: usize,
        question: u32,
        answer: &str,
        reason: Option<ReviewReason>,
    ) -> Result<(), AppError> {
        if let Some(r) = &reason {
            r.validate().map_err(AppError::Review)?;
        }
        let def = self.session.definition.clone();
        if form >= def.forms.len() || question == 0 || question > u32::from(def.questions) {
            return Err(AppError::NotFound(format!(
                "form {form} question {question}"
            )));
        }
        if self.session.key_unconfirmed() {
            return Err(AppError::Review(format!(
                "the key comes from key sheets that are not confirmed yet: decide question \
                 {question} when you confirm them (omr key confirm --answer {question}={answer}), \
                 or give the whole key as a file (omr key replace)"
            )));
        }
        let q = question as usize - 1;
        let cell: KeyCell = parse_key_entry(answer, def.options)
            .map_err(AppError::Review)?
            .into();
        let Some(key) = self.session.key.clone() else {
            return Err(AppError::Scoring(
                "the exam has no key yet; supply the whole key first (omr key replace --key \
                 key.csv)"
                    .into(),
            ));
        };
        let was_missing = key.forms[form][q] == KeyCell::Missing;
        let r = reason.unwrap_or_else(|| {
            ReviewReason::code(if was_missing {
                "CLERICAL_CORRECTION"
            } else {
                "KEY_ERROR"
            })
        });
        let given = self.stored_reason(&r)?;
        self.change_key_entry(form, q, cell, &given)?;
        self.save()?;
        Ok(())
    }

    /// One key entry changed (`form` 0-based, `q` 0-based) as a new key version: rescored
    /// without re-projecting, published with its reason (`KeyEntryChanged`), the release
    /// superseded; not saved. Shared items change on every form.
    fn change_key_entry(
        &mut self,
        form: usize,
        q: usize,
        cell: KeyCell,
        reason: &omr_store::Reason,
    ) -> Result<u32, AppError> {
        let def = self.session.definition.clone();
        if self.session.key_unconfirmed() {
            return Err(AppError::Review(format!(
                "the key comes from key sheets that are not confirmed yet: decide question {} \
                 when you confirm them (omr key confirm), or give the whole key as a file (omr \
                 key replace)",
                q + 1
            )));
        }
        let Some(mut key) = self.session.key.clone() else {
            return Err(AppError::Scoring(
                "the exam has no key yet; supply the whole key first (omr key replace --key \
                 key.csv)"
                    .into(),
            ));
        };
        if form >= key.forms.len() || q >= key.forms[form].len() {
            return Err(AppError::NotFound(format!(
                "form {form} question {}",
                q + 1
            )));
        }
        if key.shared_items {
            for f in key.forms.iter_mut() {
                f[q] = cell.clone();
            }
        } else {
            key.forms[form][q] = cell;
        }
        key.version += 1;
        // Rescore first, then commit: a refused change leaves the key in force untouched.
        match self.scoring.as_mut() {
            Some(s) => {
                s.set_key(&def, &key)?;
                self.session.key = Some(key.clone());
            }
            None => {
                self.session.key = Some(key.clone());
                self.scoring = Scoring::build(&self.session)?;
            }
        }
        // A key read from key sheets and corrected after its confirmation: the correction is
        // the instructor's decision on that row, so a later re-confirmation (a rescan of the
        // key sheet in an appended batch drops the confirmation) keeps it, shown as decided,
        // instead of reverting silently to what the sheet shows.
        let ks = &mut self.session.key_sheets;
        if ks.any() && !ks.superseded {
            let text = key.forms[form][q].text();
            let forms: Vec<usize> = if key.shared_items {
                (0..key.forms.len()).collect()
            } else {
                vec![form]
            };
            for f in forms {
                ks.decisions.push(crate::model::KeyRowDecision {
                    form: f,
                    question: q,
                    answer: text.clone(),
                });
            }
        }
        self.publish_key_with(
            "KeyEntryChanged",
            json!({"form": form as u64 + 1, "question": q as u64 + 1, "key_version": key.version}),
            reason,
        )?;
        self.after_release("key entry changed")?;
        Ok(key.version)
    }

    /// Publishes the key in force as a new key version with its reason (`KeyVersionPublished`,
    /// payload hash only) and a change event.
    fn publish_key(
        &mut self,
        ty: &str,
        payload: serde_json::Value,
        reason: &ReviewReason,
    ) -> Result<(), AppError> {
        let r = self.stored_reason(reason)?;
        self.publish_key_with(ty, payload, &r)
    }

    /// [`Self::publish_key`] with the reason in its store form.
    fn publish_key_with(
        &mut self,
        ty: &str,
        payload: serde_json::Value,
        r: &omr_store::Reason,
    ) -> Result<(), AppError> {
        let exam = self.session.definition.exam_id.to_string();
        let r = r.clone();
        if let Some(st) = self.store.as_mut() {
            if let Some(sc) = &self.scoring {
                st.put_scoring_setup(sc)?;
                st.put_key_version(&sc.key, &r)?;
            }
            st.event_with_reason(ty, "exam", &exam, payload, &r)?;
        }
        Ok(())
    }

    /// Replaces the whole answer key (a new key version, full rescore; §10.1, §10.8): e.g.
    /// the real key after an inferred-key run, or a corrected key file. `forms` holds one key
    /// per form of the exam (any order); `points`, when given, replaces the points per
    /// question. The SCO-003 `inferred_key` blocker clears (its item is resolved with the
    /// reason, default `KEY_ERROR`) and booklets whose form was inferred are re-inferred under
    /// the new key (their open form items get the new proposal). The forms keep their item
    /// layout: a key that gives identical forms different answers on an exam graded with one
    /// shared item set needs a regrade.
    pub fn replace_key(
        &mut self,
        forms: Vec<crate::exam::FormKey>,
        points: Option<Vec<String>>,
        reason: Option<ReviewReason>,
    ) -> Result<(), AppError> {
        let points = points.map(|p| crate::exam::KeyFilePoints {
            all: Some(p),
            per_form: Default::default(),
        });
        self.replace_key_points(forms, points.as_ref(), reason)
    }

    /// [`Self::replace_key`] with the points of a key file as read by
    /// [`crate::exam::read_key_file_full`]: a `points` column for every version and/or one
    /// per version (`points_A`, `points_B`). `None` keeps the exam's points.
    pub fn replace_key_points(
        &mut self,
        forms: Vec<crate::exam::FormKey>,
        points: Option<&crate::exam::KeyFilePoints>,
        reason: Option<ReviewReason>,
    ) -> Result<(), AppError> {
        let reason = reason.unwrap_or_else(|| ReviewReason::code("KEY_ERROR"));
        reason.validate().map_err(AppError::Review)?;
        let mut def = self.session.definition.clone();
        def.key = crate::exam::KeySource::Provided { forms };
        if let Some(p) = points {
            def.set_key_points(p);
        }
        def.validate()?;
        let crate::exam::KeySource::Provided { forms } = &def.key else {
            unreachable!("set above");
        };
        let unconfirmed = self.session.key_unconfirmed();
        // An unconfirmed key-sheet key was never published: the file's key takes its number.
        let version = match &self.session.key {
            Some(k) if unconfirmed => k.version,
            Some(k) => k.version + 1,
            None => 1,
        };
        let mut key = crate::pipeline::provided_key(&def, forms, version)?;
        if let Some(old) = self.session.key.as_ref().filter(|_| !unconfirmed) {
            if old.shared_items && !key.shared_items {
                return Err(AppError::Scoring(
                    "the new key gives the forms different answers or points, but the exam was \
                     graded with one item set shared by every form; grade the exam again with \
                     this key (omr grade --key)"
                        .into(),
                ));
            }
            // Keep the item layout the exam was graded (and stored) with.
            key.shared_items = old.shared_items;
        }
        // Graded without any key, booklets of a multi-form exam with no declared form were
        // neither inferred nor queued: only a regrade with the key can infer and queue them.
        let undeclared = self
            .session
            .booklets
            .iter()
            .filter(|b| b.form.is_none())
            .count();
        if self.session.key.is_none() && def.forms.len() > 1 && undeclared > 0 {
            return Err(AppError::Scoring(format!(
                "the exam was graded without a key and {undeclared} booklet(s) declare no form; \
                 grade it again with this key (omr grade --key) so their forms are inferred and \
                 reviewed"
            )));
        }
        let was_inferred = self.session.key.as_ref().is_some_and(|k| k.inferred);
        let mut session = self.session.clone();
        session.definition = def;
        session.key = Some(key.clone());
        // A key file replaces key sheets (§10.1): the hold lifts, the sheets stay listed.
        let hold_rows = if session.key_sheets.any() && !session.key_sheets.superseded {
            session.key_sheets.superseded = true;
            crate::keysheet::refresh_pending(&mut session.key_sheets);
            session
                .exam_exceptions
                .retain(|e| e.sub_reason.as_deref() != Some(crate::keysheet::HOLD));
            std::mem::take(&mut session.key_sheets.hold_rows)
        } else {
            Vec::new()
        };
        refresh_inferred_forms(&mut session, &key);
        // Rescore first, then commit: a refused key leaves the exam untouched.
        let scoring = Scoring::build(&session)?;
        self.session = session;
        self.scoring = scoring;
        // Inferred forms may have moved under the new key.
        self.touch_all();
        if was_inferred {
            self.session.exam_exceptions.retain(|e| {
                !(e.code == "SCO-003" && e.sub_reason.as_deref() == Some("inferred_key"))
            });
            let ids: Vec<u64> = self
                .session
                .review
                .items
                .iter()
                .filter(|i| {
                    i.is_open()
                        && i.code == "SCO-003"
                        && i.sub_reason.as_deref() == Some("inferred_key")
                })
                .map(|i| i.id)
                .collect();
            let r = if ids.is_empty() {
                None
            } else {
                Some(self.stored_reason(&reason)?)
            };
            for id in ids {
                let rows = {
                    let it = self.session.review.item_mut(id).expect("listed");
                    it.state = ItemState::Resolved;
                    it.resolution = Some(Decision::Acknowledge);
                    it.store_exceptions.clone()
                };
                if let (Some(st), Some(r)) = (self.store.as_mut(), &r) {
                    st.resolve(&rows, r, "key_replaced")?;
                }
            }
        }
        if !hold_rows.is_empty() {
            let r = self.stored_reason(&reason)?;
            if let Some(st) = self.store.as_mut() {
                st.resolve(&hold_rows, &r, "key_file_replaces_key_sheets")?;
            }
        }
        self.publish_key(
            "KeyReplaced",
            json!({"key_version": version, "forms": self.session.definition.forms.len(), "replaces_inferred": was_inferred, "replaces_key_sheets": !hold_rows.is_empty()}),
            &reason,
        )?;
        self.after_release("key replaced")?;
        self.save()?;
        Ok(())
    }

    /// The instructor confirms the key read from key sheets (§10.1: key-sheet responses are
    /// never auto-accepted; the instructor MUST confirm the rendered key summary,
    /// [`Self::key_sheet_summary`]). `version` confirms one version (form index), `None` every
    /// version read and not yet confirmed. `decisions` settle rows first (several marks, blank,
    /// an option not used, a mark not read confidently, sheets that disagree: `B`, `A|C` to
    /// accept both explicitly, `*` to void); a version with a row left undecided is refused,
    /// naming each row and what to write (R10).
    ///
    /// When the last waiting version is confirmed the hold (SCO-003 `key_sheet_unconfirmed`)
    /// lifts: the key is published as a key version whose `KeyVersionPublished` event names the
    /// source (`key_sheet`), every scan's SHA-384 and the confirming operator; inferred forms
    /// are re-inferred under it and the exam is scored. A key sheet that cannot be read holds
    /// the run until it is rescanned or a key file replaces the key ([`Self::replace_key`]).
    pub fn confirm_key_sheets(
        &mut self,
        version: Option<usize>,
        decisions: Vec<crate::model::KeyRowDecision>,
        reason: Option<ReviewReason>,
    ) -> Result<crate::keysheet::KeyConfirmReport, AppError> {
        use crate::keysheet::{HOLD, key_cells, open_rows, refresh_pending};
        if let Some(r) = &reason {
            r.validate().map_err(AppError::Review)?;
        }
        let def = self.session.definition.clone();
        let n = usize::from(def.questions);
        if !self.session.key_unconfirmed() {
            return Err(AppError::Review(
                if self.session.key_sheets.any() {
                    "the key from the key sheets is already confirmed (or a key file replaced \
                     it); correct one entry with omr key set"
                } else {
                    "this exam has no key sheets to confirm"
                }
                .into(),
            ));
        }
        let mut ks = self.session.key_sheets.clone();
        let forms = ks.forms();
        let name = |f: usize| def.forms.get(f).cloned().unwrap_or_default();
        let targets: Vec<usize> = match version {
            Some(v) if !forms.contains(&v) => {
                return Err(AppError::Review(format!(
                    "no readable key sheet for version {}: the versions read from key sheets are \
                     {}",
                    name(v),
                    forms
                        .iter()
                        .map(|&f| name(f))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            Some(v) if ks.is_confirmed(v) => {
                return Err(AppError::Review(format!(
                    "version {} is already confirmed",
                    name(v)
                )));
            }
            Some(v) => vec![v],
            None => forms
                .iter()
                .copied()
                .filter(|f| !ks.is_confirmed(*f))
                .collect(),
        };
        if targets.is_empty() {
            let why: Vec<String> = ks.sheets.iter().filter_map(|k| k.refusal.clone()).collect();
            return Err(AppError::Review(format!(
                "every readable key sheet is confirmed, but the run stays held: {}",
                why.join("; ")
            )));
        }
        for d in &decisions {
            if !forms.contains(&d.form) || ks.is_confirmed(d.form) {
                return Err(AppError::Review(format!(
                    "--answer for version {}: only rows of a key-sheet version that is not \
                     confirmed yet can be decided here (change a confirmed key with omr key set)",
                    name(d.form)
                )));
            }
            if d.question >= n {
                return Err(AppError::Review(format!(
                    "--answer {}: the exam has questions 1–{n}",
                    d.question + 1
                )));
            }
            match parse_key_entry(&d.answer, def.options) {
                Ok(crate::exam::ParsedKey::Missing) => {
                    return Err(AppError::Review(format!(
                        "--answer {}={}: give an answer (B), several accepted answers (A|C) or \
                         * to void the question",
                        d.question + 1,
                        d.answer
                    )));
                }
                Ok(_) => {}
                Err(m) => {
                    return Err(AppError::Review(format!(
                        "--answer {}={}: {m}",
                        d.question + 1,
                        d.answer
                    )));
                }
            }
        }
        ks.decisions.extend(decisions.iter().cloned());
        let mut open = Vec::new();
        for &f in &targets {
            for (q, why) in open_rows(&ks, f) {
                open.push(format!(
                    "version {} question {}: {why} (--answer {}=…)",
                    name(f),
                    q + 1,
                    q + 1
                ));
            }
        }
        if !open.is_empty() {
            return Err(AppError::Review(format!(
                "the key cannot be confirmed yet: {} row(s) need your decision: {}. Decide each \
                 one with --answer QUESTION=ANSWER (B; A|C to accept both; * to void){} and \
                 confirm again",
                open.len(),
                open.join("; "),
                if forms.len() > 1 && version.is_none() {
                    " together with --version"
                } else {
                    ""
                }
            )));
        }
        let by = self.store.as_ref().map_or_else(
            || "principal:instructor".to_string(),
            |s| s.actor().id.clone(),
        );
        let at = crate::pipeline::now_utc();
        for &f in &targets {
            let scans: Vec<(String, u32)> = ks
                .sheets
                .iter()
                .filter(|k| k.usable() && k.form == Some(f))
                .flat_map(|k| k.pages.iter())
                .map(|&pi| {
                    let p = &self.session.pages[pi];
                    (self.session.sources[p.source].sha384.clone(), p.index + 1)
                })
                .collect();
            ks.confirmed.push(crate::model::KeyConfirmation {
                form: f,
                by: by.clone(),
                at_utc: at.clone(),
                scans,
            });
        }
        refresh_pending(&mut ks);
        let mut session = self.session.clone();
        let mut key = session.key.clone().ok_or_else(|| {
            AppError::Internal("a key-sheet key waits for confirmation but no key is set".into())
        })?;
        for &f in &targets {
            if let Some(cells) = key_cells(&def, &ks, f)
                && let Some(slot) = key.forms.get_mut(f)
            {
                *slot = cells;
            }
        }
        let published: Vec<u32> = match self.store.as_ref() {
            Some(st) => st
                .reader()
                .key_versions(&def.exam_id)
                .map_err(crate::store::store_error)?,
            None => Vec::new(),
        };
        let lifted = !ks.pending;
        if lifted {
            if !key.inferred {
                key.shared_items = key.can_share_items(&def);
            }
            if let Some(&top) = published.iter().max()
                && key.version <= top
            {
                key.version = top + 1;
            }
        }
        session.key_sheets = ks;
        session.key = Some(key.clone());
        let hold_rows = if lifted {
            session
                .exam_exceptions
                .retain(|e| e.sub_reason.as_deref() != Some(HOLD));
            refresh_inferred_forms(&mut session, &key);
            std::mem::take(&mut session.key_sheets.hold_rows)
        } else {
            for e in session.exam_exceptions.iter_mut() {
                if e.sub_reason.as_deref() == Some(HOLD) {
                    *e = crate::keysheet::hold_exception(&def, &session.key_sheets);
                }
            }
            Vec::new()
        };
        // Score first, then commit: a refused key leaves the exam untouched.
        let scoring = Scoring::build(&session)?;
        // The publish checks, printed and shown on screens: a letter pattern of the key
        // (SCO-027) is named by version and p only, never by its letter counts (C10); the full
        // detail is in the key summary page.
        let warnings: Vec<String> = scoring
            .as_ref()
            .map(|sc| {
                let mut w: Vec<String> = sc
                    .key
                    .validate(&sc.forms)
                    .into_iter()
                    .filter(|e| {
                        !matches!(
                            e.sub_reason.as_deref(),
                            Some("letter_run" | "letter_distribution")
                        )
                    })
                    .map(|e| format!("{} {:?}: {}", e.code, e.severity, e.detail))
                    .collect();
                w.extend(sc.key.letter_patterns(&sc.forms).iter().map(|pat| {
                    let f = usize::from(pat.form().0).saturating_sub(1);
                    pat.public_text(def.forms.get(f).map_or("?", String::as_str))
                }));
                w
            })
            .unwrap_or_default();
        self.session = session;
        self.scoring = scoring;
        self.touch_all();
        let confirm_reason = reason
            .clone()
            .unwrap_or_else(|| ReviewReason::code("ADJUDICATION"));
        let r = self.stored_reason(&confirm_reason)?;
        let stored_reason = match &reason {
            Some(x) => Some(self.stored_reason(x)?),
            None => None,
        };
        let exam = def.exam_id.to_string();
        let provenance = crate::keysheet::provenance(&self.session, &by);
        let mapping = def
            .outcomes
            .as_ref()
            .zip(self.scoring.as_ref())
            .map(|(o, sc)| {
                crate::outcomes::mapping_set(
                    &def,
                    o,
                    self.session.key.as_ref(),
                    sc.forms.items.len(),
                )
            });
        if let Some(st) = self.store.as_mut() {
            let forms_json: Vec<String> = targets.iter().map(|&f| name(f)).collect();
            if let Some(sc) = self.scoring.as_ref() {
                st.put_scoring_setup(sc)?;
                st.put_key_version_from(&sc.key, stored_reason.as_ref(), &provenance)?;
                if let Some(m) = &mapping {
                    st.put_mapping_set(m)?;
                }
            } else {
                st.event_with_reason(
                    "KeySheetVersionConfirmed",
                    "exam",
                    &exam,
                    json!({"forms": forms_json, "source": "key_sheet", "pending": self.session.key_sheets.pending}),
                    &r,
                )?;
            }
            st.resolve(&hold_rows, &r, "key_sheet_confirmed")?;
        }
        self.after_release("key confirmed from key sheets")?;
        self.save()?;
        Ok(crate::keysheet::KeyConfirmReport {
            confirmed: targets.iter().map(|&f| name(f)).collect(),
            waiting: self
                .session
                .key_sheets
                .forms()
                .into_iter()
                .filter(|f| !self.session.key_sheets.is_confirmed(*f))
                .map(name)
                .collect(),
            held: self.session.key_sheets.pending,
            key_version: lifted.then_some(key.version),
            warnings,
        })
    }

    /// Sets or clears (`None`) the weight of question `question` (1-based) toward CLO `clo`
    /// (index), `0 < w ≤ 1`; outcomes recompute on the next query (§11.2).
    pub fn set_mapping_weight(
        &mut self,
        question: u16,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), AppError> {
        refuse_per_version_mapping_edit(&self.session.definition)?;
        let def = &mut self.session.definition;
        if question == 0 || question > def.questions {
            return Err(AppError::NotFound(format!("question {question}")));
        }
        let o = def
            .outcomes
            .as_mut()
            .ok_or_else(|| AppError::Review("the exam has no CLOs".into()))?;
        let code = o
            .clos
            .get(clo)
            .map(|c| c.code.clone())
            .ok_or_else(|| AppError::NotFound(format!("CLO {clo}")))?;
        let pos = o
            .mapping
            .iter()
            .position(|m| m.question == question && m.clo == code);
        match weight {
            None => {
                if let Some(i) = pos {
                    o.mapping.remove(i);
                }
            }
            Some(w) if w > 0.0 && w <= 1.0 && w.is_finite() => {
                let text = format!("{:.4}", w)
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string();
                match pos {
                    Some(i) => o.mapping[i].weight = text,
                    None => o.mapping.push(crate::exam::CloMapping {
                        question,
                        clo: code.clone(),
                        weight: text,
                        note: String::new(),
                        form: String::new(),
                        imported: String::new(),
                        approved_by: String::new(),
                    }),
                }
            }
            Some(w) => {
                return Err(AppError::Review(format!("weight {w} outside 0 < w ≤ 1")));
            }
        }
        let exam = def.exam_id.to_string();
        if let Some(st) = self.store.as_mut() {
            st.event(
                "MappingChanged",
                "exam",
                &exam,
                json!({"question": question, "clo": code, "weight": weight.map(|w| format!("{w}"))}),
            )?;
        }
        self.save()
    }

    /// Approves the imported item → CLO rows (§11.2: an approval act by a coordinator- or
    /// QA-scoped role; "each approval is an audit event"): every row imported from another
    /// system (a Remark learning objective, `imported, needs review`) and not yet approved
    /// becomes `approved` by `approver`, its source kept; a `MappingApproved` event names the
    /// approver, the rows and the CLOs (the note as a salted reference), and the mapping set is
    /// published again. The TP-154 tables then leave the draft watermark (SCO-032) without the
    /// exam being graded again. Refused with what to do when nothing is waiting for approval.
    /// Returns the rows approved.
    pub fn approve_mappings(&mut self, approver: &str, note: &str) -> Result<usize, AppError> {
        self.check_audited()?;
        let approver = approver.trim();
        if approver.is_empty() {
            return Err(AppError::Review(
                "name the approver (the course coordinator or the quality officer who reviewed \
                 the mapping), e.g. --approver \"Dr. A. Coordinator\""
                    .into(),
            ));
        }
        let o = self.session.definition.outcomes.as_mut().ok_or_else(|| {
            AppError::Review(
                "the exam has no CLO mapping to approve; give one with omr clo replace --clo FILE"
                    .into(),
            )
        })?;
        let mut clos: Vec<String> = Vec::new();
        let mut rows = 0usize;
        for m in o
            .mapping
            .iter_mut()
            .filter(|m| !m.imported.is_empty() && m.approved_by.is_empty())
        {
            m.approved_by = approver.to_string();
            rows += 1;
            if !clos.contains(&m.clo) {
                clos.push(m.clo.clone());
            }
        }
        if rows == 0 {
            return Err(AppError::Review(
                "no imported CLO mapping is waiting for approval (every row is approved or the \
                 coordinator's own)"
                    .into(),
            ));
        }
        let reason = ReviewReason::with_note("ADJUDICATION", note);
        let given = self.stored_reason(&reason)?;
        let exam = self.session.definition.exam_id.to_string();
        let mapping = self.mapping_set_now();
        if let Some(st) = self.store.as_mut() {
            st.event_with_reason(
                "MappingApproved",
                "exam",
                &exam,
                json!({"approver": approver, "rows": rows, "clos": clos, "status": "approved"}),
                &given,
            )?;
            if let Some(m) = &mapping {
                st.put_mapping_set(m)?;
            }
        }
        self.save()?;
        Ok(rows)
    }

    /// Replaces the exam's CLOs and item → CLO mapping with a CLO file (the `--clo` layout,
    /// [`crate::exam::read_clo_csv`]), e.g. the imported `clo.csv` once the coordinator has
    /// corrected it: the outcomes recompute from the same score run, nothing is graded again.
    /// Its `form` column must name versions of this exam; rows for one version only are
    /// refused when the versions share one item set (grade the exam again with `--clo`). A
    /// `MappingReplaced` event names the file's SHA-384 and the counts, and the mapping set is
    /// published. Imported rows (`imported, needs review`) stay `proposed` until approved
    /// ([`Self::approve_mappings`]). Returns (CLOs, rows).
    pub fn replace_outcomes(
        &mut self,
        bytes: &[u8],
        file: &str,
    ) -> Result<(usize, usize), AppError> {
        self.check_audited()?;
        let def = &self.session.definition;
        let o = crate::exam::read_clo_csv(bytes, def.questions)?;
        if let Some(m) = o
            .mapping
            .iter()
            .find(|m| !m.form.is_empty() && !def.forms.contains(&m.form))
        {
            return Err(AppError::Input(format!(
                "{file}: question {} maps version {}, and the exam has versions {}: correct the \
                 form column",
                m.question,
                m.form,
                def.forms.join(", ")
            )));
        }
        if self.session.key.as_ref().is_some_and(|k| k.shared_items)
            && def.forms.len() > 1
            && o.mapping.iter().any(|m| !m.form.is_empty())
        {
            return Err(AppError::Input(format!(
                "{file} maps questions of one version only (the form column), and this exam \
                 scores its versions as one item set: grade the exam again with --clo {file}, \
                 or map every version alike (leave the form column empty)"
            )));
        }
        let mut d = def.clone();
        d.outcomes = Some(o.clone());
        d.validate()?;
        let (clos, rows) = (o.clos.len(), o.mapping.len());
        let proposed = o.mapping.iter().filter(|m| !m.imported.is_empty()).count();
        self.session.definition.outcomes = Some(o);
        let exam = self.session.definition.exam_id.to_string();
        let mapping = self.mapping_set_now();
        if let Some(st) = self.store.as_mut() {
            st.event(
                "MappingReplaced",
                "exam",
                &exam,
                json!({
                    "file": file,
                    "sha384": omr_store::jcs::sha384_hex(bytes),
                    "clos": clos,
                    "rows": rows,
                    "proposed": proposed,
                }),
            )?;
            if let Some(m) = &mapping {
                st.put_mapping_set(m)?;
            }
        }
        self.save()?;
        Ok((clos, rows))
    }

    /// The current item → CLO mapping set (§11.2), when the exam has CLOs and a score run.
    fn mapping_set_now(&self) -> Option<omr_outcomes::MappingSet> {
        let def = &self.session.definition;
        def.outcomes
            .as_ref()
            .zip(self.scoring.as_ref())
            .map(|(o, sc)| {
                crate::outcomes::mapping_set(
                    def,
                    o,
                    self.session.key.as_ref(),
                    sc.forms.items.len(),
                )
            })
    }

    /// Sets the note of a mapping cell (the cell must exist).
    pub fn set_mapping_note(
        &mut self,
        question: u16,
        clo: usize,
        note: &str,
    ) -> Result<(), AppError> {
        refuse_per_version_mapping_edit(&self.session.definition)?;
        let o = self
            .session
            .definition
            .outcomes
            .as_mut()
            .ok_or_else(|| AppError::Review("the exam has no CLOs".into()))?;
        let code = o
            .clos
            .get(clo)
            .map(|c| c.code.clone())
            .ok_or_else(|| AppError::NotFound(format!("CLO {clo}")))?;
        let m = o
            .mapping
            .iter_mut()
            .find(|m| m.question == question && m.clo == code)
            .ok_or_else(|| AppError::NotFound(format!("mapping Q{question} → {code}")))?;
        m.note = note.to_string();
        self.save()
    }

    /// Item analysis of the current run (§12): the synchronous path (≤ 50 ms, never a
    /// bootstrap). Once the flag-stability job has run for this score run
    /// ([`Self::flag_stability`], e.g. on a background thread after a key change), its result is
    /// merged: each rank 1–4 flag carries its stability share, and a flag that fires in fewer
    /// than half of the resamples or fails at one end of its CI is labelled low confidence
    /// *(proposed)* (§12.2). Before that the flags carry no stability ("computing…").
    pub fn analytics(&self) -> Option<AnalyticsReport> {
        let s = self.scoring.as_ref()?;
        let run = s.run.as_ref().ok()?;
        let data = AnalysisData::from_run(run, &s.prep.matrix);
        let mut report = omr_analytics::analyze(&data, &AnalyticsConfig::default());
        let cached = self
            .stability
            .lock()
            .expect("stability lock")
            .clone()
            .filter(|st| st.run_id == data.run_id && st.n == data.n);
        if let Some(st) = cached {
            omr_analytics::apply_stability(&mut report, st);
        }
        Some(report)
    }

    /// The flag-stability job (§12.2 "Flag stability", §12.3 "Bootstrap") for the current
    /// score run: N < 1,000 → the share of 1,000 bootstrap resamples (seeded from the score-run
    /// ID, so the same run gives the same shares on any machine and thread count) in which each
    /// rank 1–4 flag fires again; N ≥ 1,000 → both ends of the Fisher-z CI. Computed once per
    /// score run and kept, so [`Self::analytics`] merges it; a key change or review decision
    /// makes a new score run, and the job runs again on the next call. Off the ≤ 50 ms path:
    /// the exports call it, and the desktop app may call it on a background thread.
    pub fn flag_stability(&self) -> Option<omr_analytics::StabilityReport> {
        let s = self.scoring.as_ref()?;
        let run = s.run.as_ref().ok()?;
        let data = AnalysisData::from_run(run, &s.prep.matrix);
        if let Some(st) = self
            .stability
            .lock()
            .expect("stability lock")
            .as_ref()
            .filter(|st| st.run_id == data.run_id && st.n == data.n)
        {
            return Some(st.clone());
        }
        let cfg = AnalyticsConfig::default();
        let report = omr_analytics::analyze(&data, &cfg);
        let st = omr_analytics::stability_job(&data, &report.flags, &cfg);
        *self.stability.lock().expect("stability lock") = Some(st.clone());
        Some(st)
    }

    /// Item analysis with flag stability merged (the exports): [`Self::flag_stability`], then
    /// [`Self::analytics`].
    pub fn analytics_with_stability(&self) -> Option<AnalyticsReport> {
        self.flag_stability()?;
        self.analytics()
    }

    /// The current score run's ID, when the exam is scored.
    pub fn current_run_id(&self) -> Option<String> {
        self.scoring
            .as_ref()
            .and_then(|s| s.run.as_ref().ok())
            .map(|r| r.run_id.clone())
    }

    /// The release gate's mis-key check (§12.2, §8.1): the current score run's item analysis
    /// with its flag stability ([`Self::analytics_with_stability`]; the bootstrap job runs here
    /// when it has not run for this score run), then every probable mis-key at normal
    /// confidence becomes (or stays) a Scoring-lane SCO-001 ERROR item that blocks release
    /// until the instructor decides it ([`crate::review::sync_flag_items`]); a low-confidence
    /// flag stays a non-blocking SCO-002 `proposed_miskey` of the item analysis. An inferred
    /// or unconfirmed key is not judged. Until this has run for the current score run the
    /// gate reads "computing…". The exports and the release run it; the desktop runs it when
    /// its background stability job reports ([`Self::set_flag_stability`]).
    pub fn judge_flags(&mut self) -> Result<(), AppError> {
        let run = self.current_run_id();
        if run.is_some() && self.session.flags.judged_run == run {
            return Ok(());
        }
        let judged = self.session.key.as_ref().is_some_and(|k| !k.inferred)
            && !self.session.key_unconfirmed();
        let report = if judged {
            self.analytics_with_stability()
        } else {
            None
        };
        crate::review::sync_flag_items(&mut self.session, report.as_ref());
        self.session.flags.judged_run = run.clone();
        self.session.flags.current_run = run;
        self.save()
    }

    /// Takes the result of a flag-stability job run off this thread (the desktop's background
    /// job, §12.3) for the score run it was computed on, then judges the flags
    /// ([`Self::judge_flags`]) without running the job again. A report for an older score run
    /// is ignored.
    pub fn set_flag_stability(
        &mut self,
        report: omr_analytics::StabilityReport,
    ) -> Result<(), AppError> {
        if self.current_run_id().as_deref() != Some(report.run_id.as_str()) {
            return Ok(());
        }
        *self.stability.lock().expect("stability lock") = Some(report);
        self.judge_flags()
    }

    /// Learning outcomes of the current run (§11).
    pub fn outcomes(&self) -> Option<OutcomeReport> {
        crate::outcomes::compute(&self.session, self.scoring.as_ref()?)
    }

    fn recognizer(&self) -> Result<Arc<Recognizer>, AppError> {
        let mut g = self.recognizer.lock().expect("recognizer lock");
        if let Some(r) = g.as_ref() {
            return Ok(r.clone());
        }
        let r = Arc::new(Recognizer::new(&self.session.definition)?);
        *g = Some(r.clone());
        Ok(r)
    }

    fn source(&self, s: usize) -> Result<Arc<SourceFile>, AppError> {
        if let Some(f) = self.sources.lock().expect("sources lock").get(&s) {
            return Ok(f.clone());
        }
        let info = self
            .session
            .sources
            .get(s)
            .ok_or_else(|| AppError::NotFound(format!("source {s}")))?;
        let st = self
            .store
            .as_ref()
            .ok_or_else(|| AppError::NotFound("no project store for page images".into()))?;
        let blake = info
            .blake3
            .as_deref()
            .ok_or_else(|| AppError::NotFound(format!("{}: original not stored", info.name)))?;
        let bytes = st.read_object(blake, &info.sha384)?;
        let ext = match info.kind {
            ingest::SourceKind::Pdf => "pdf",
            ingest::SourceKind::Image => "png",
        };
        let f = SourceFile::from_bytes(Path::new(&format!("x.{ext}")), bytes)
            .ok_or_else(|| AppError::Internal("source kind".into()))?;
        let f = Arc::new(f);
        self.sources
            .lock()
            .expect("sources lock")
            .insert(s, f.clone());
        Ok(f)
    }

    /// The working-resolution image of a page (re-decoded from the stored original; a small
    /// cache keeps the last pages for prefetch, §13.2).
    pub fn page_image(&self, page: usize) -> Result<Arc<GrayImage>, AppError> {
        if let Some((_, img)) = self
            .pages
            .lock()
            .expect("page cache")
            .iter()
            .find(|(p, _)| *p == page)
        {
            return Ok(img.clone());
        }
        let p = self
            .session
            .pages
            .get(page)
            .ok_or_else(|| AppError::NotFound(format!("page {page}")))?;
        let src = self.source(p.source)?;
        let (open, _) = ingest::open_source(&src);
        let d = ingest::decode_page(&src, &open, p.index, &p.page_id)
            .map_err(|e| AppError::Input(e.detail))?;
        let rec = self.recognizer()?;
        let w = rec.to_working(d.image, d.capture, d.known_dpi);
        let img = Arc::new(w.image);
        let mut cache = self.pages.lock().expect("page cache");
        cache.push((page, img.clone()));
        if cache.len() > 12 {
            cache.remove(0);
        }
        Ok(img)
    }

    /// RGBA crop of a review item (registered region with its bubble geometry).
    pub fn crop(&self, item: u64) -> Result<crate::crops::Crop, AppError> {
        let it = self
            .session
            .review
            .item(item)
            .ok_or_else(|| AppError::NotFound(format!("review item {item}")))?;
        match &it.crop {
            Some(c) => {
                let img = self.page_image(c.page)?;
                Ok(crate::crops::render(&img, c, it.kind != ItemKind::Identity))
            }
            None => {
                let page = it
                    .page
                    .ok_or_else(|| AppError::NotFound(format!("item {item} has no page")))?;
                let img = self.page_image(page)?;
                Ok(crate::crops::thumbnail(&img, 900))
            }
        }
    }

    /// Appends a later batch of scans to this exam, e.g. a second section scanned on another
    /// day (§8.1, §13.4). The exam is graded again over every stored original plus the new
    /// files, so batch baselines, collation, serials, form inference, an inferred key and the
    /// queue are exactly those of grading everything at once; a supplied or corrected key
    /// stays in force and every review decision carries over by stable key (a re-run that
    /// contradicts a human value raises SYS-032 and keeps the human value). Audited: a new scan
    /// batch and new recognition and identity runs in the same store; the earlier run's open
    /// exceptions are superseded, renumbered serials and moved pages are logged. A file that
    /// was already imported is CAP-011 (byte-identical) and adds nothing. Earlier review events
    /// stay in the history but can no longer be undone. `opts.threads` and
    /// `opts.snapshot_utc` apply (the exam's snapshot time by default); the package is this
    /// project's. A run that fails or is cancelled (`opts.cancel`) after its spool leaves the
    /// exam's results as they were but keeps the spool: its batch stays open in the log, and the
    /// originals and page rows it stored stay; adding the same files again adds their pages
    /// once (they keep that batch's ID).
    pub fn add_scans(
        &mut self,
        inputs: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<crate::append::AppendReport, AppError> {
        if self.session.key_unconfirmed() {
            return Err(AppError::Review(
                "the key read from the key sheets is not confirmed: confirm it first (omr key \
                 confirm), or give the key as a file (omr key replace), then add the scans"
                    .into(),
            ));
        }
        let paths = ingest::collect_paths(inputs)?;
        let def = self.session.definition.clone();
        self.regrade(def, &paths, opts, progress, "scans added")
    }

    /// The exam graded again under `def` over every stored original plus the files `paths`
    /// (none: the same scans, e.g. under a new roster), as [`Self::add_scans`] describes; a
    /// release is superseded naming `change`. The exam changes only when the run succeeds.
    fn regrade(
        &mut self,
        def: ExamDefinition,
        paths: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
        change: &str,
    ) -> Result<crate::append::AppendReport, AppError> {
        use rayon::prelude::*;
        let pool = crate::pipeline::pool(opts.threads)?;
        let Some(store) = self.store.take() else {
            return Err(AppError::Store(
                "this exam was opened read-only; open it for writing (omr add --project DIR) \
                 to add scans"
                    .into(),
            ));
        };
        let root = store.root();
        let actor = store.actor().clone();
        let prior = self.session.clone();
        let mut sources = match crate::append::stored_sources(&prior, &store) {
            Ok(s) => s,
            Err(e) => {
                self.store = Some(store);
                return Err(e);
            }
        };
        let known: std::collections::BTreeSet<&str> =
            prior.sources.iter().map(|s| s.sha384.as_str()).collect();
        let new: Vec<Result<SourceFile, (PathBuf, String)>> = pool.install(|| {
            paths
                .par_iter()
                .map(|p| SourceFile::read(p).map_err(|e| (p.clone(), e)))
                .collect()
        });
        let duplicates = new
            .iter()
            .filter(|f| f.as_ref().is_ok_and(|f| known.contains(f.sha384.as_str())))
            .count();
        let files = new.len();
        sources.extend(new);
        let mut o = opts.clone();
        o.project_dir = None;
        if o.snapshot_utc.is_none() {
            o.snapshot_utc = Some(prior.snapshot_utc.clone());
        }
        let g = crate::pipeline::grade_into(
            &def,
            sources,
            &o,
            &pool,
            std::time::Instant::now(),
            progress,
            Some(crate::pipeline::Append {
                store,
                prior: &prior,
            }),
        );
        let g = match g {
            Ok(g) => g,
            Err(e) => {
                // The store went into the failed run: reopen it so the exam stays usable.
                if let Ok(mut st) = StoreWriter::open(&root, &actor.id) {
                    st.set_actor(&actor.id, &actor.role);
                    self.store = Some(st);
                }
                return Err(e);
            }
        };
        let before: std::collections::BTreeSet<&str> =
            prior.pages.iter().map(|p| p.page_id.as_str()).collect();
        let s = &g.session;
        let report = crate::append::AppendReport {
            batch: g
                .store
                .as_ref()
                .map(|st| st.batch_id().to_string())
                .unwrap_or_default(),
            files,
            new_pages: s
                .pages
                .iter()
                .filter(|p| {
                    !before.contains(p.page_id.as_str())
                        && p.status != crate::model::PageStatus::Duplicate
                })
                .count(),
            duplicates,
            booklets_before: prior.booklets.len(),
            booklets_after: s.booklets.len(),
            carried_over: s
                .review
                .items
                .iter()
                .filter(|i| i.state == ItemState::Resolved)
                .count(),
            conflicts: s
                .review
                .items
                .iter()
                .filter(|i| i.is_open() && i.code == "SYS-032")
                .count(),
        };
        let release = self.session.release.take();
        self.session = g.session;
        self.session.release = release;
        self.scoring = g.scoring;
        self.store = g.store;
        self.after_release(change)?;
        self.touch_all();
        self.sources.lock().expect("sources lock").clear();
        self.pages.lock().expect("page cache").clear();
        // Pages changed: the page snapshot is written again.
        if let Some(st) = &self.store {
            let _ = std::fs::remove_file(st.cache_dir().join(PAGES_FILE));
        }
        self.bound = None;
        self.save()?;
        self.judge_flags()?;
        Ok(report)
    }

    /// Key sheets added to a graded exam (§10.1, C7): the instructor's filled answer sheets,
    /// named as key sheets (their SHA-384 join the definition's declared key sheets), are
    /// graded with the exam as a later batch ([`Self::add_scans`]): their originals are stored
    /// and hashed like every scan, they hold the run until the instructor confirms the rendered
    /// key summary ([`Self::confirm_key_sheets`]), and the published key version names them
    /// (`KeyVersionPublished { source: key_sheet, scans }`). A file already graded as a student
    /// (one that bubbled the marker) is graded again as the key sheet. This is the audited path
    /// for a key read from answer sheets after grading (the desktop's *Read key from answer
    /// sheet…*); `keysheet::read_key_sheets` only previews.
    pub fn add_key_sheets(
        &mut self,
        files: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<crate::append::AppendReport, AppError> {
        let paths = ingest::collect_paths(files)?;
        let mut shas = Vec::new();
        for p in &paths {
            let f = SourceFile::read(p).map_err(|e| {
                AppError::Input(format!(
                    "key sheet {}: {e}; check the path or rescan the key sheet",
                    p.display()
                ))
            })?;
            shas.push(f.sha384);
        }
        let before = self.session.definition.key_sheets.declared.clone();
        let declared = &mut self.session.definition.key_sheets.declared;
        declared.extend(shas.iter().cloned());
        declared.sort();
        declared.dedup();
        if let Some(st) = self.store.as_mut() {
            let exam = self.session.definition.exam_id.to_string();
            st.event(
                "KeySheetsAdded",
                "exam",
                &exam,
                json!({"files": paths.len(), "sha384": shas}),
            )?;
        }
        match self.add_scans(&paths, opts, progress) {
            Ok(r) => Ok(r),
            Err(e) => {
                self.session.definition.key_sheets.declared = before;
                Err(e)
            }
        }
    }

    /// A roster given to a graded exam, or replacing its roster (§9.1: a rosterless exam is
    /// converted by a normal linkage run; §9.6: assignment runs again after a new roster
    /// snapshot). The rows are imported as `--roster` imports them
    /// ([`ExamDefinition::import_roster`]: leading zeros restored; duplicate, damaged or
    /// national IDs refuse the roster), then every booklet is identified again over the stored
    /// originals ([`Self::add_scans`] without new scans): booklets, scores, the queue and the
    /// exports are those of grading with this roster from the start, every review decision
    /// carries over (a re-run that contradicts a reviewer's student raises SYS-032 and keeps
    /// it) and a release is superseded. Audited by the run's store stage, in one decision with
    /// the identity run that names it: the roster snapshot (its parent the roster it replaces)
    /// and `RosterReplaced` (row count and hashes, no student data). The exam row keeps the
    /// identity mode it was created with; the roster in force is the latest identity run's.
    /// Refused while the key read from key sheets waits for confirmation; a refused roster, or
    /// a run that fails or is cancelled, leaves the exam and its definition as they were and
    /// records no roster (a run stopped after its spool leaves the scan batch it opened open
    /// in the log; it spools no page).
    pub fn set_roster(
        &mut self,
        rows: Vec<crate::exam::RosterStudent>,
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<crate::append::RosterReport, AppError> {
        if self.session.key_unconfirmed() {
            return Err(AppError::Review(
                "the key read from the key sheets is not confirmed: confirm it first (omr key \
                 confirm), or give the key as a file (omr key replace), then add the roster"
                    .into(),
            ));
        }
        let Some(st) = self.store.as_mut() else {
            return Err(AppError::Store(
                "this exam was opened read-only; open it for writing to add the roster".into(),
            ));
        };
        let mut def = self.session.definition.clone();
        // Student numbers masked: the notes reach logs and the status line.
        let warnings: Vec<String> = def
            .import_roster(rows)?
            .iter()
            .map(|w| crate::export::mask_numbers(w))
            .collect();
        st.stage_roster(&def);
        let report = match self.regrade(def, &[], opts, progress, "roster replaced") {
            Ok(r) => r,
            Err(e) => {
                // A run that failed before its store stage recorded no roster.
                if let Some(st) = self.store.as_mut() {
                    st.unstage_roster();
                }
                return Err(e);
            }
        };
        let s = &self.session;
        let matched = s.booklets.iter().filter(|b| b.student().is_some()).count();
        Ok(crate::append::RosterReport {
            students: s.definition.roster.as_ref().map_or(0, Vec::len),
            matched,
            not_on_roster: s.booklets.len() - matched,
            absent: self.absent_students().len(),
            carried_over: report.carried_over,
            conflicts: report.conflicts,
            warnings,
        })
    }

    /// Releases the results (§10.8, §13.4): refused while anything blocks release (an open
    /// ERROR/BLOCKER item, an inferred or incomplete key; R10: the reason and what to do).
    /// The exam is checked against its store, the score run is stored and frozen, a
    /// `ResultsReleased` event names it with its results hash, and the chain is checkpointed.
    /// Only released results are written to the Blackboard upload; a later change supersedes
    /// the release (it must be released again).
    pub fn release(&mut self, reason: Option<ReviewReason>) -> Result<(), AppError> {
        if let Some(r) = &reason {
            r.validate().map_err(AppError::Review)?;
        }
        // The mis-key check of the current results (§12.2) runs first: a probable mis-key
        // must be decided before release.
        self.judge_flags()?;
        if let Some(why) = self.session.release_blocked() {
            return Err(AppError::Review(format!(
                "release refused: {why}. Resolve the open items (omr review list) or supply the \
                 key (omr key replace), then release again"
            )));
        }
        if self.session.released() {
            return Err(AppError::Review(
                "the results are already released and nothing changed since".into(),
            ));
        }
        self.check_audited()?;
        let run = match self.scoring.as_ref().map(|s| &s.run) {
            Some(Ok(run)) => run.clone(),
            _ => {
                return Err(AppError::Scoring(
                    "release refused: there is no score run (no key, or the key leaves items \
                     unscored); supply the key first"
                        .into(),
                ));
            }
        };
        let results = omr_store::jcs::sha384_hex(
            &serde_json::to_vec(&run).map_err(|e| AppError::Internal(e.to_string()))?,
        );
        let by = self
            .store
            .as_ref()
            .map_or_else(|| "principal:local".to_string(), |s| s.actor().id.clone());
        let exam = self.session.definition.exam_id.to_string();
        let given = match &reason {
            Some(r) => Some(self.stored_reason(r)?),
            None => None,
        };
        if let Some(st) = self.store.as_mut() {
            st.put_score_run(&run)?;
            let payload = json!({
                "score_run": run.run_id,
                "key_version": run.key_version,
                "results_sha384": results,
            });
            match &given {
                Some(r) => st.event_with_reason("ResultsReleased", "exam", &exam, payload, r)?,
                None => st.event("ResultsReleased", "exam", &exam, payload)?,
            }
        }
        self.session.release = Some(crate::model::Release {
            at_utc: crate::pipeline::now_utc(),
            by,
            score_run: run.run_id.clone(),
            key_version: run.key_version,
            results_sha384: results,
            superseded: None,
        });
        self.save()?;
        if let Some(st) = self.store.as_mut() {
            st.checkpoint()?;
        }
        Ok(())
    }

    /// A change after the release (§10.8): the release is superseded (`ReleaseSuperseded`,
    /// naming what changed), the Blackboard upload is no longer written, and the results must
    /// be released again.
    fn after_release(&mut self, what: &str) -> Result<(), AppError> {
        if !self.session.released() {
            return Ok(());
        }
        let exam = self.session.definition.exam_id.to_string();
        if let Some(st) = self.store.as_mut() {
            st.event("ReleaseSuperseded", "exam", &exam, json!({"change": what}))?;
        }
        if let Some(r) = self.session.release.as_mut() {
            r.superseded = Some(what.to_string());
        }
        Ok(())
    }

    /// Queues the settled differences of a parallel run with the incumbent (§23.7: every
    /// per-question difference from the engine's auto-accepted response is adjudicated by the
    /// instructor before release). Each question this exam auto-accepted where the incumbent
    /// read otherwise becomes a Marks-lane MRK-001 `parallel_run_disagreement` (ERROR) item with
    /// both readings and its bubble row: the answer waits for review, so the booklet's total is
    /// provisional and release is held until the instructor decides it. Accepting keeps this
    /// exam's reading; choosing the incumbent's or another answer needs a reason (§13.4); each
    /// decision is recorded as a field label (`FieldLabelRecorded`: engine loss, incumbent
    /// loss). A question queued this way once is not queued again. The comparison is an audited
    /// event (`ParallelRunCompared`). Refused on released results: a correction after release
    /// is a review decision or a key change, which supersedes the release (§10.8). Returns the
    /// number of new items, also recorded in `run` for its report.
    pub fn queue_parallel_differences(
        &mut self,
        run: &mut crate::parallel::ParallelRun,
    ) -> Result<usize, AppError> {
        use crate::parallel::{Agreement, PARALLEL_SUB};
        use crate::review::{IncumbentReading, suggestion_of};
        self.check_audited()?;
        if self.session.released() {
            return Err(AppError::Review(
                "the results are already released: the parallel run's differences stay in its \
                 report and are not queued; correct an answer with a review decision after \
                 release, which supersedes the release (§10.8), then release again"
                    .into(),
            ));
        }
        let def = &self.session.definition;
        let labels = def.option_labels();
        let options = def.options;
        let mut queued: Vec<u64> = Vec::new();
        let mut touched: Vec<usize> = Vec::new();
        for d in run
            .disagreements
            .iter()
            .filter(|d| d.agreement == Agreement::Differ && d.status == "auto")
        {
            let Some(pair) = run.pairs.get(d.pair) else {
                continue;
            };
            let (bi, q) = (pair.booklet, usize::from(d.question.saturating_sub(1)));
            let Some(qs) = self
                .session
                .booklets
                .get(bi)
                .and_then(|b| b.questions.get(q))
            else {
                continue;
            };
            let already = self.session.review.items.iter().any(|i| {
                i.kind == ItemKind::Mark
                    && i.booklet == Some(bi)
                    && i.question == Some(q as u16)
                    && (i.is_open() || i.sub_reason.as_deref() == Some(PARALLEL_SUB))
            });
            if already || qs.status != ResponseStatus::Auto {
                continue;
            }
            let Some(engine) = suggestion_of(&qs.current) else {
                continue;
            };
            let incumbent = match &d.remark_answer {
                crate::remark::RemarkAnswer::Option(o) if *o < options => {
                    Some(Decision::Option(*o))
                }
                crate::remark::RemarkAnswer::Blank => Some(Decision::Blank),
                crate::remark::RemarkAnswer::Multi(v) if v.iter().all(|o| *o < options) => {
                    Some(Decision::Multi(v.clone()))
                }
                _ => None,
            };
            let ours = engine.text(&labels, &def.forms);
            let e = omr_model::Exception::must("MRK-001", Severity::Error, omr_model::Scope::Item)
                .with_sub_reason(PARALLEL_SUB)
                .with_detail(format!(
                    "parallel run (§23.7): Remark Office OMR read {}, this exam auto-accepted \
                     {ours}; decide from the bubble row: accept keeps {ours}, or choose Remark's \
                     or another answer with a reason (e.g. AMBIGUOUS_MARK); the decision is \
                     recorded as a field label",
                    d.remark
                ));
            let mut it = crate::review::mark_item(&self.session, bi, q, &e, &labels);
            it.id = self.session.review.items.len() as u64 + 1;
            it.suggestion = Some(engine.clone());
            it.incumbent = Some(IncumbentReading {
                system: "Remark Office OMR".into(),
                text: d.remark.clone(),
                decision: incumbent,
                engine,
                source: d.remark_row.clone(),
            });
            queued.push(it.id);
            self.session.review.items.push(it);
            self.session.booklets[bi].questions[q].status = ResponseStatus::Pending;
            touched.push(bi);
        }
        touched.sort_unstable();
        touched.dedup();
        for &bi in &touched {
            if let Some(sc) = self.scoring.as_mut() {
                sc.update_booklet(bi, &self.session.booklets[bi])?;
            }
            self.touch(bi);
        }
        let exam = self.session.definition.exam_id.to_string();
        let c = &run.counts;
        if let Some(st) = self.store.as_mut() {
            st.event(
                "ParallelRunCompared",
                "exam",
                &exam,
                json!({
                    "incumbent": "Remark Office OMR",
                    "workbooks": run.import.workbooks.iter().map(|w| w.sha384.clone()).collect::<Vec<_>>(),
                    "booklets": c.booklets,
                    "questions": c.questions,
                    "differ": c.differ,
                    "differ_pending": c.differ_pending,
                    "queued": queued,
                }),
            )?;
        }
        run.queued = Some(queued.len());
        run.counts.not_queued = run.counts.not_queued.saturating_sub(queued.len());
        run.counts.held_in_review += queued.len();
        self.save()?;
        Ok(queued.len())
    }

    /// Writes the review crops of `items` into `dir` as a Restricted export
    /// ([`crate::export::write_crops`]) and records `ExportCreated` (purpose `review-crops`).
    pub fn export_crops(
        &mut self,
        dir: &Path,
        items: &[u64],
    ) -> Result<crate::export::ExportSummary, AppError> {
        self.check_audited()?;
        let summary = crate::export::write_crops(self, dir, items)?;
        if let Some(st) = self.store.as_mut() {
            st.record_export(
                "review-crops",
                &format!("folder:{}", dir.display()),
                &summary.manifest_sha384,
            )?;
        }
        Ok(summary)
    }

    /// Records an export of this exam's data written outside the exam folder (`ExportCreated`,
    /// label `restricted`, PII), e.g. the parallel-run report.
    pub(crate) fn record_export(
        &mut self,
        purpose: &str,
        destination: &str,
        manifest_sha384: &str,
    ) -> Result<(), AppError> {
        if let Some(st) = self.store.as_mut() {
            st.record_export(purpose, destination, manifest_sha384)?;
        }
        Ok(())
    }

    /// Records that personal data was shown unmasked outside the exports (`PersonalDataViewed`:
    /// the purpose and how many records, never the data), e.g. `omr review list --reveal`.
    pub fn record_reveal(&mut self, purpose: &str, records: usize) -> Result<(), AppError> {
        let exam = self.session.definition.exam_id.to_string();
        if let Some(st) = self.store.as_mut() {
            st.event(
                "PersonalDataViewed",
                "exam",
                &exam,
                json!({"purpose": purpose, "records": records}),
            )?;
        }
        Ok(())
    }

    /// Writes the Blackboard Learn grade-center upload to `path` ([`crate::export::blackboard_csv`]),
    /// optionally for an existing grade column (`column_id`, the `|835932` of its downloaded
    /// header); records `ExportCreated`. Refused (with what to do) when the exam must not be
    /// pushed: an inferred key, no roster or no score run.
    pub fn export_blackboard(
        &mut self,
        path: &Path,
        column_id: Option<&str>,
    ) -> Result<(), AppError> {
        self.check_audited()?;
        let Some(bytes) = crate::export::blackboard_csv(self, column_id)? else {
            return Err(AppError::Scoring(
                "no Blackboard file: grades are pushed only when released (omr release), with a \
                 supplied key (not inferred), a roster and a score run; supply them (omr key \
                 replace, --roster) and release first"
                    .into(),
            ));
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            crate::keys::export_dir(dir)?;
        }
        crate::keys::write_private(path, &bytes)?;
        if let Some(st) = self.store.as_mut() {
            st.record_export(
                "lms_gradebook",
                &format!("file:{}", path.display()),
                &omr_store::jcs::sha384_hex(&bytes),
            )?;
        }
        Ok(())
    }

    /// Checks the exam against its store before anything leaves the project (§14.4): the
    /// responses, students and key that decide the grades must be the audited ones (SYS-011).
    pub fn check_audited(&self) -> Result<(), AppError> {
        let Some(st) = &self.store else {
            return Ok(());
        };
        let upto = st.snapshot_binding()?.map(|b| b.seq);
        crate::audit::against_store(
            &self.session,
            &st.reader(),
            st.exam(),
            Some(st.recog_run()),
            upto,
        )
    }

    /// Writes every export to `dir` (see [`crate::export`]) after checking the exam against
    /// its store; the chain is checkpointed (signed, and kept outside the package).
    pub fn export(&mut self, dir: &Path) -> Result<crate::export::ExportSummary, AppError> {
        self.save()?;
        // The item analysis the exports carry decides the mis-key check too (§12.2).
        self.judge_flags()?;
        self.check_audited()?;
        // The manifest carries the signed checkpoint of the state it was written from.
        if let Some(st) = self.store.as_mut() {
            st.checkpoint()?;
        }
        let summary = crate::export::write_all(self, dir)?;
        self.session.timings.analysis_ms = summary.analysis_ms;
        self.session.timings.export_ms = summary.total_ms;
        if let Some(st) = self.store.as_mut() {
            if let Some(Ok(run)) = self.scoring.as_ref().map(|s| &s.run) {
                st.put_score_run(run)?;
            }
            st.record_export(
                "grades",
                &format!("folder:{}", dir.display()),
                &summary.manifest_sha384,
            )?;
        }
        self.save()?;
        if let Some(st) = self.store.as_mut() {
            st.checkpoint()?;
        }
        Ok(summary)
    }
}

/// A mapping cell names a printed question and a CLO; when the versions map their printed
/// questions differently (rows with a `form`, e.g. Remark learning objectives imported per
/// version, §3.5), one cell stands for several rows and editing it would change one version's
/// row silently: refused with the way to edit them.
fn refuse_per_version_mapping_edit(def: &crate::exam::ExamDefinition) -> Result<(), AppError> {
    if def.has_form_mapping() {
        return Err(AppError::Review(
            "this exam's CLO mappings differ between versions (rows per version, e.g. imported \
             from Remark): a cell of the grid stands for several rows, so it is not edited here; \
             change the rows in clo.csv (its `form` column names each row's version) and grade \
             the exam again with it (omr grade … --clo clo.csv)"
                .into(),
        ));
    }
    Ok(())
}

/// The longest operator name recorded in the audit trail, in characters.
pub const OPERATOR_NAME_MAX: usize = 64;

/// The audit-trail principal (`principal:<name>`) of the name an operator gives for themself:
/// trimmed, inner runs of spaces collapsed to one, 1 to [`OPERATOR_NAME_MAX`] characters, with
/// no control character (a tab, a line break) and no `:` (the principal's own separator). A name
/// given as a principal (`principal:Huda A`) is read as the name after the prefix. Shells and the
/// command line record decisions under what this returns, so every event names its actor alike.
pub fn operator_principal(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    let name = name.strip_prefix("principal:").unwrap_or(name);
    if name.chars().any(char::is_control) {
        return Err(AppError::Input(
            "a name cannot hold a tab, a line break or another control character".into(),
        ));
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(AppError::Input(format!(
            "type the name to record (1 to {OPERATOR_NAME_MAX} characters)"
        )));
    }
    if name.contains(':') {
        return Err(AppError::Input(
            "a name cannot hold a colon (:); it separates the parts of the recorded actor".into(),
        ));
    }
    let n = name.chars().count();
    if n > OPERATOR_NAME_MAX {
        return Err(AppError::Input(format!(
            "a name has at most {OPERATOR_NAME_MAX} characters; this one has {n}"
        )));
    }
    Ok(format!("principal:{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_names_become_principals_or_are_refused() {
        let ok = |n: &str| operator_principal(n).expect(n);
        assert_eq!(ok("Huda A"), "principal:Huda A");
        assert_eq!(
            ok("  Huda   A  "),
            "principal:Huda A",
            "trimmed, spaces collapsed"
        );
        assert_eq!(
            ok("principal:Huda A"),
            "principal:Huda A",
            "a principal is its name"
        );
        assert_eq!(ok("هدى العتيبي"), "principal:هدى العتيبي");
        assert_eq!(ok("Dr. O'Neil-Smith"), "principal:Dr. O'Neil-Smith");
        let longest = "x".repeat(OPERATOR_NAME_MAX);
        assert_eq!(ok(&longest), format!("principal:{longest}"));
        // 64 characters, not bytes: an Arabic name of 64 letters is 128 bytes
        assert!(operator_principal(&"ه".repeat(OPERATOR_NAME_MAX)).is_ok());
        for bad in [
            "",
            "   ",
            "principal:",
            "Huda: A",
            "principal:engine:recog",
            "Huda\tA",
            "Huda\u{7}",
            "Huda\nA",
        ] {
            let e = operator_principal(bad).expect_err(bad);
            assert!(matches!(e, AppError::Input(_)), "{bad:?}: {e}");
        }
        let e = operator_principal(&"x".repeat(OPERATOR_NAME_MAX + 1)).unwrap_err();
        assert!(e.to_string().contains("this one has 65"), "{e}");
    }
}
