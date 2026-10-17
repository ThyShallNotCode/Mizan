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
    /// that fails leaves no half-made project behind (the package it created is removed), so
    /// the same `--out` folder can be used again.
    pub fn grade(
        def: &ExamDefinition,
        inputs: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<Self, AppError> {
        let fresh = opts.project_dir.as_ref().filter(|d| !d.exists()).cloned();
        // The exam folder that will hold the package and its exports is private to the user
        // when the app creates it (Restricted data, §19.3).
        if let Some(parent) = fresh
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| !d.as_os_str().is_empty() && !d.exists())
        {
            crate::keys::create_private_dir(parent)?;
        }
        let result = crate::pipeline::grade(def, inputs, opts, progress).and_then(|g| {
            let mut p = Self::from_graded(g);
            p.save()?;
            Ok(p)
        });
        if result.is_err()
            && let Some(dir) = fresh
        {
            let _ = std::fs::remove_dir_all(dir);
        }
        result
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
        let q = question as usize - 1;
        let cell: KeyCell = parse_key_entry(answer, def.options)
            .map_err(AppError::Review)?
            .into();
        let Some(mut key) = self.session.key.clone() else {
            return Err(AppError::Scoring(
                "the exam has no key yet; supply the whole key first (omr key replace --key \
                 key.csv)"
                    .into(),
            ));
        };
        let was_missing = key.forms[form][q] == KeyCell::Missing;
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
        let r = reason.unwrap_or_else(|| {
            ReviewReason::code(if was_missing {
                "CLERICAL_CORRECTION"
            } else {
                "KEY_ERROR"
            })
        });
        self.publish_key(
            "KeyEntryChanged",
            json!({"form": form as u64 + 1, "question": question, "key_version": key.version}),
            &r,
        )?;
        self.after_release("key entry changed")?;
        self.save()?;
        Ok(())
    }

    /// Publishes the key in force as a new key version with its reason (`KeyVersionPublished`,
    /// payload hash only) and a change event.
    fn publish_key(
        &mut self,
        ty: &str,
        payload: serde_json::Value,
        reason: &ReviewReason,
    ) -> Result<(), AppError> {
        let exam = self.session.definition.exam_id.to_string();
        let r = self.stored_reason(reason)?;
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
        let reason = reason.unwrap_or_else(|| ReviewReason::code("KEY_ERROR"));
        reason.validate().map_err(AppError::Review)?;
        let mut def = self.session.definition.clone();
        def.key = crate::exam::KeySource::Provided { forms };
        if let Some(p) = points {
            def.points = p;
        }
        def.validate()?;
        let crate::exam::KeySource::Provided { forms } = &def.key else {
            unreachable!("set above");
        };
        let version = self.session.key.as_ref().map_or(1, |k| k.version + 1);
        let mut key = crate::pipeline::provided_key(&def, forms, version)?;
        if let Some(old) = &self.session.key {
            if old.shared_items && !key.shared_items {
                return Err(AppError::Scoring(
                    "the new key gives the forms different answers, but the exam was graded \
                     with one item set shared by every form; grade the exam again with this \
                     key (omr grade --key)"
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
        self.publish_key(
            "KeyReplaced",
            json!({"key_version": version, "forms": self.session.definition.forms.len(), "replaces_inferred": was_inferred}),
            &reason,
        )?;
        self.after_release("key replaced")?;
        self.save()?;
        Ok(())
    }

    /// Sets or clears (`None`) the weight of question `question` (1-based) toward CLO `clo`
    /// (index), `0 < w ≤ 1`; outcomes recompute on the next query (§11.2).
    pub fn set_mapping_weight(
        &mut self,
        question: u16,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), AppError> {
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

    /// Sets the note of a mapping cell (the cell must exist).
    pub fn set_mapping_note(
        &mut self,
        question: u16,
        clo: usize,
        note: &str,
    ) -> Result<(), AppError> {
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

    /// Item analysis of the current run (§12).
    pub fn analytics(&self) -> Option<AnalyticsReport> {
        let s = self.scoring.as_ref()?;
        let run = s.run.as_ref().ok()?;
        Some(omr_analytics::analyze(
            &AnalysisData::from_run(run, &s.prep.matrix),
            &AnalyticsConfig::default(),
        ))
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
    /// project's.
    pub fn add_scans(
        &mut self,
        inputs: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<crate::append::AppendReport, AppError> {
        use rayon::prelude::*;
        let paths = ingest::collect_paths(inputs)?;
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
        let pool = crate::pipeline::pool(opts.threads)?;
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
        let def = prior.definition.clone();
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
        self.after_release("scans added")?;
        self.touch_all();
        self.sources.lock().expect("sources lock").clear();
        self.pages.lock().expect("page cache").clear();
        // Pages changed: the page snapshot is written again.
        if let Some(st) = &self.store {
            let _ = std::fs::remove_file(st.cache_dir().join(PAGES_FILE));
        }
        self.bound = None;
        self.save()?;
        Ok(report)
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
