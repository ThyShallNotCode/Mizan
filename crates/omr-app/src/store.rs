//! The project store side of the pipeline (§14): the exam package, spool-first originals,
//! recognition, identity, exceptions, score runs and review events, all through `omr-store`
//! (hash-chained events). The writer is used from the scheduler thread only (one writer, §4.5).

use crate::AppError;
use crate::exam::ExamDefinition;
use crate::ingest::SourceFile;
use crate::keys;
use crate::model::{PageStatus, Session, SourceInfo};
use crate::review::ItemKind;
use crate::score::Scoring;
use omr_model::{EngineManifest, Exception, Response};
use omr_store::{
    Actor, Anonymity, AssessmentKind, BatchRecord, BookletRecord, ExamRecord, ExportRecord,
    IdentityDecisionRecord, IdentityMode, IdentityRunRecord, IdentityStatus, NewEvent, NewProject,
    ObjectKind, OpenOptions, PageRecognition, PageRecord, PageState, PinnedKind, Project, Reason,
    RecognitionRunRecord, ReviewAction, RosterRowIn, RosterSnapshotIn, RunKind, StagedObject,
    SystemClock, TemplateOrigin,
};
use rayon::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Batch ID of a grading run.
pub const BATCH_ID: &str = "batch-1";
/// Recognition run ID.
pub const RECOG_RUN: &str = "recog-final-1";
/// Identity run ID.
pub const IDENTITY_RUN: &str = "identity-1";
/// Session snapshot file name (in the package's `cache/`).
pub const SESSION_FILE: &str = "session.json";
/// Page snapshot file name (in the package's `cache/`).
pub const PAGES_FILE: &str = "pages.json";

/// A booklet of the earlier run: its serial and its (page ID, template page) list.
type StoredBooklet = (u32, Vec<(String, u16)>);

/// Open exception rows by (page ID, question): (row ID, code, sub-reason).
type PageQuestionRows = BTreeMap<(String, Option<u16>), Vec<(i64, String, Option<String>)>>;

/// The open project and what the pipeline staged into it.
pub struct StoreWriter {
    project: Project,
    exam: omr_model::ExamId,
    actor: Actor,
    staged: Vec<Option<StagedObject>>,
    /// Batch, recognition and identity run of the grading run being written (the first run
    /// uses [`BATCH_ID`], [`RECOG_RUN`], [`IDENTITY_RUN`]; each appended batch the next number).
    batch_id: String,
    recog_run: String,
    identity_run: String,
    parent_run: Option<String>,
    /// The roster snapshot in force (SHA-384): the one the latest identity run used.
    roster_snapshot: Option<String>,
    /// A roster given after grading, recorded with the next identity run (its store stage).
    staged_roster: Option<RosterSnapshotIn>,
}

/// The roster snapshot of `def`'s roster (§9.2), `parent` the one it replaces.
fn roster_snapshot_in(def: &ExamDefinition, parent: Option<String>) -> Option<RosterSnapshotIn> {
    def.roster.as_ref().map(|rows| RosterSnapshotIn {
        parent,
        source: "import".into(),
        fetched_at: crate::pipeline::now_utc(),
        id_scheme_version: format!("{:?}", def.id_scheme),
        rows: rows
            .iter()
            .map(|r| RosterRowIn {
                student_id: r.id.clone(),
                section: r.section.clone(),
                sis_status: "enrolled".into(),
                fields: json!({"name": r.name, "name_ar": r.name_ar}),
                attributes: BTreeMap::new(),
            })
            .collect(),
    })
}

impl std::fmt::Debug for StoreWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StoreWriter({:?})", self.project.paths().root)
    }
}

/// Event recording the SHA-384 of the session snapshot as saved (`cache/session.json`,
/// `cache/pages.json`): the snapshot is a cache bound to the chain, never an unauthenticated
/// source of grades (§14.4).
pub const SNAPSHOT_EVENT: &str = "SnapshotSaved";

/// What the chain says the session snapshot is (its latest [`SNAPSHOT_EVENT`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotBinding {
    /// SHA-384 (hex) of `session.json`.
    pub session_sha384: String,
    /// SHA-384 (hex) of `pages.json`.
    pub pages_sha384: String,
    /// SHA-384 (hex) of the booklet chunk index (`booklets.json`; absent in a snapshot
    /// written before chunks).
    pub index_sha384: Option<String>,
    /// SHA-384 (hex) of the page-code secret file (`secrets/page-code-key.hex`) of a
    /// generated sheet.
    pub secret_sha384: Option<String>,
    /// The event's position in the chain.
    pub seq: i64,
}

/// The package's private folder (mode 0700) for the exam's page-code secret.
pub const SECRETS_DIR: &str = "secrets";
/// The page-code secret K_exam (hex), mode 0600 (§6.4, §19.3; sealing under a project key is
/// deferred, §19.2).
pub const PAGE_CODE_KEY_FILE: &str = "page-code-key.hex";

/// The latest snapshot binding recorded in a project, if any (a project written before
/// snapshots were bound has none).
pub fn snapshot_binding(
    reader: &omr_store::Reader<'_>,
) -> Result<Option<SnapshotBinding>, AppError> {
    let Some(ev) = reader.last_event(SNAPSHOT_EVENT).map_err(store_err)? else {
        return Ok(None);
    };
    let field = |k: &str| {
        ev.payload[k]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AppError::Store(format!("{SNAPSHOT_EVENT} event {} lacks {k}", ev.seq)))
    };
    Ok(Some(SnapshotBinding {
        session_sha384: field("session_sha384")?,
        pages_sha384: field("pages_sha384")?,
        index_sha384: ev.payload["booklets_index_sha384"]
            .as_str()
            .map(str::to_string),
        secret_sha384: ev.payload["secret_sha384"].as_str().map(str::to_string),
        seq: ev.seq,
    }))
}

/// The project UUID named by a package's marker, if readable.
fn marker_uuid(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join("project.omr")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v["project_uuid"].as_str().map(str::to_string)
}

/// How omr-app opens a package (§14.2, §14.4): SYS-015 location checks on (sync folders and
/// network mounts refused), full chain and projection verification, the device key signing
/// checkpoints, and this device's latest checkpoint of the project as an external anchor.
fn open_options(dir: &Path, read_only: bool) -> Result<OpenOptions, AppError> {
    let signer = keys::device_signer()?;
    let verifier = keys::DeviceVerifier::new(&signer)?;
    let external = match marker_uuid(dir) {
        Some(u) => keys::external_checkpoint(&u, &verifier)?
            .into_iter()
            .collect(),
        None => Vec::new(),
    };
    Ok(OpenOptions {
        device_id: "device:omr-app".into(),
        clock: Box::new(SystemClock),
        check_location: true,
        lock: !read_only,
        verify_chain: true,
        verifier: Some(Box::new(keys::DeviceVerifier::new(&signer)?)),
        external,
        signer: (!read_only)
            .then(|| Box::new(signer) as Box<dyn omr_store::CheckpointSigner + Send>),
    })
}

/// Opens a package read-only with every check of [`StoreWriter::open`] (no lock, no
/// writes): the session snapshot's binding, for viewers and dashboards.
pub fn open_read_only(dir: &Path) -> Result<omr_store::ReadOnlyProject, AppError> {
    Project::open_read_only(dir, open_options(dir, true)?).map_err(store_err)
}

/// A store failure in the user's terms: what happened, its Appendix A code and what to do
/// (R10).
pub fn store_error(e: omr_store::StoreError) -> AppError {
    store_err(e)
}

fn store_err(e: omr_store::StoreError) -> AppError {
    use omr_store::StoreError as E;
    let code = e.exception().map(|x| x.code);
    let remedy = match &e {
        E::Chain(omr_store::ChainError::Truncated { .. }) => {
            "the audit log is shorter than this device's last checkpoint of the project: events \
             were removed, or this is an older copy of the project. The project is read-only \
             and release is blocked; open the current copy, or restore it from your backup, \
             and report it to the exam's QA officer"
        }
        E::Chain(_) => {
            "the audit log was changed outside the app (or is damaged). The project is \
             refused: restore the project folder from your backup or the last signed export, \
             and report it to the exam's QA officer"
        }
        E::Integrity(_) => {
            "the project database is damaged: restore the project folder from your backup, or \
             from its snapshots/ folder"
        }
        E::UnsafeLocation(_) => {
            "projects and exports hold Restricted student data and must stay on a local disk \
             outside sync and network folders (PDPL): choose a local folder outside Dropbox, \
             OneDrive, Nextcloud, iCloud or a network share"
        }
        E::Locked(_) => {
            "another program has the exam open for writing (an `omr` command or a desktop \
             window): close it and try again"
        }
        _ => "",
    };
    let mut msg = match code {
        Some(c) => format!("{c} {e}"),
        None => e.to_string(),
    };
    if !remedy.is_empty() {
        msg.push_str("; ");
        msg.push_str(remedy);
    }
    AppError::Store(msg)
}

impl StoreWriter {
    /// Creates the package at `dir` (refused when it already holds a project).
    pub fn create(
        dir: &Path,
        def: &ExamDefinition,
        _snapshot_utc: &str,
        operator: &str,
    ) -> Result<Self, AppError> {
        let project = Project::create(
            dir,
            NewProject {
                project_uuid: None,
                assessment_id: Some(def.exam_id.to_string()),
                ..NewProject::default()
            },
            open_options(dir, false)?,
        )
        .map_err(store_err)?;
        Ok(Self::with(project, def.exam_id, operator))
    }

    fn with(project: Project, exam: omr_model::ExamId, operator: &str) -> Self {
        Self {
            project,
            exam,
            actor: Actor::user(operator, "Instructor"),
            staged: Vec::new(),
            batch_id: BATCH_ID.into(),
            recog_run: RECOG_RUN.into(),
            identity_run: IDENTITY_RUN.into(),
            parent_run: None,
            roster_snapshot: None,
            staged_roster: None,
        }
    }

    /// Opens an existing package (location, lock, chain, projections and this device's
    /// external checkpoint verified); review decisions go to its latest identity run. The exam
    /// is the one the package was created for.
    pub fn open(dir: &Path, operator: &str) -> Result<Self, AppError> {
        let project = Project::open(dir, open_options(dir, false)?).map_err(store_err)?;
        let exam = project
            .header()
            .assessment_id
            .as_deref()
            .and_then(|a| uuid::Uuid::parse_str(a).ok())
            .map(omr_model::ExamId)
            .ok_or_else(|| {
                AppError::Store(format!(
                    "SYS-010 {}: the package names no exam; restore it from your backup",
                    dir.display()
                ))
            })?;
        let mut w = Self::with(project, exam, operator);
        let events = w.events()?;
        let last = |ty: &str| {
            events
                .iter()
                .rev()
                .find(|e| e.event_type == ty)
                .map(|e| e.entity_id.clone())
        };
        if let Some(b) = last("BatchOpened") {
            w.batch_id = b;
        }
        if let Some(r) = last("RecognitionRunStarted") {
            w.recog_run = r;
        }
        if let Some(r) = last("IdentityRunStarted") {
            w.identity_run = r;
        }
        // The roster in force: the one the latest identity run used; a package whose identity
        // runs do not name it (graded before they did) has at most its setup's roster.
        w.roster_snapshot = events
            .iter()
            .rev()
            .find(|e| e.event_type == "IdentityRunStarted")
            .and_then(|e| e.payload["roster_snapshot"].as_str().map(str::to_string))
            .or_else(|| {
                events
                    .iter()
                    .find(|e| e.event_type == "RosterSnapshotImported")
                    .map(|e| e.entity_id.clone())
            });
        Ok(w)
    }

    /// The scan batch of the latest grading run (e.g. `batch-2` after one appended batch).
    pub fn batch_id(&self) -> &str {
        &self.batch_id
    }

    /// Opens the next scan batch of an existing exam (`BatchOpened`), and names the next
    /// recognition and identity runs (the previous recognition run is their parent).
    pub fn begin_append(&mut self) -> Result<String, AppError> {
        let events = self.events()?;
        let n = events
            .iter()
            .filter(|e| e.event_type == "BatchOpened")
            .count()
            + 1;
        self.parent_run = events
            .iter()
            .rev()
            .find(|e| e.event_type == "RecognitionRunStarted")
            .map(|e| e.entity_id.clone());
        self.batch_id = format!("batch-{n}");
        self.recog_run = format!("recog-final-{n}");
        self.identity_run = format!("identity-{n}");
        let me = self.actor.clone();
        let exam = self.exam;
        let batch = self.batch_id.clone();
        self.project
            .decision(|tx| {
                tx.create_batch(
                    &BatchRecord {
                        batch_id: batch.clone(),
                        exam_id: exam,
                        header: None,
                        campus: None,
                        operator: Some(me.id.clone()),
                        device_profile: None,
                        expected_count: None,
                        state: "open".into(),
                    },
                    &me,
                )
            })
            .map_err(store_err)?;
        Ok(self.batch_id.clone())
    }

    /// Package root.
    pub fn root(&self) -> PathBuf {
        self.project.paths().root.clone()
    }

    /// The `cache/` folder (session snapshot).
    pub fn cache_dir(&self) -> PathBuf {
        self.project.paths().cache.clone()
    }

    /// The object store's folder, to read originals from another thread
    /// ([`ObjectStore::read_verified`](omr_store::objects::ObjectStore::read_verified)).
    pub fn objects(&self) -> omr_store::objects::ObjectStore {
        self.project.objects().clone()
    }

    /// Reads an original by its BLAKE3 address (verified).
    pub fn read_object(&self, blake3: &str, sha384: &str) -> Result<Vec<u8>, AppError> {
        self.project
            .objects()
            .read_verified(blake3, sha384)
            .map_err(store_err)
    }

    /// The acting principal.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Sets the acting principal (a reviewer).
    pub fn set_actor(&mut self, id: &str, role: &str) {
        self.actor = Actor::user(id, role);
    }

    /// Exam, template revision, roster snapshot, pins and the batch (one decision commit).
    pub fn write_setup(
        &mut self,
        def: &ExamDefinition,
        manifest: EngineManifest,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        let roster = roster_snapshot_in(def, None);
        // The exam definition is persisted with the template revision, without the roster
        // (its own snapshot) and without the page-code secret (§6.4, §19.3).
        let mut public_def = def.clone();
        public_def.roster = None;
        public_def.page_code_key_hex = None;
        if let crate::exam::KeySource::Provided { .. } = public_def.key {
            // Keys are sealed in their own key version (§19.3).
            public_def.key = crate::exam::KeySource::None;
        }
        let template_body = json!({
            "template": def.template,
            "definition": public_def,
            "roster_rows": def.roster.as_ref().map(Vec::len),
            "manifest": manifest,
        });
        // The exam row keeps the identity mode the exam was created with: a roster given after
        // grading is recorded by `RosterReplaced` and the identity runs that name its snapshot
        // (the roster in force), and nothing decides from the row.
        self.roster_snapshot = self
            .project
            .decision(|tx| {
                tx.create_exam(
                    &ExamRecord {
                        exam_id: exam,
                        kind: AssessmentKind::Exam,
                        anonymity: Anonymity::Identified,
                        identity_mode: if def.roster.is_some() {
                            IdentityMode::Roster
                        } else {
                            IdentityMode::CandidateNumber
                        },
                        title: def.title.clone(),
                        course_code: (!def.course_code.is_empty()).then(|| def.course_code.clone()),
                        term: (!def.term.is_empty()).then(|| def.term.clone()),
                        weight: None,
                        cancel_mode: Some(def.cancel_mode),
                        high_stakes: def.high_stakes,
                        label: "restricted".into(),
                    },
                    &me,
                )?;
                let origin = match def.template {
                    crate::exam::TemplateChoice::Legacy(_) => TemplateOrigin::Imported,
                    crate::exam::TemplateChoice::Generated(_) => TemplateOrigin::Native,
                };
                tx.put_template_revision(
                    &def.template.id(),
                    origin,
                    &def.template.id(),
                    1,
                    &template_body,
                    &me,
                )?;
                let roster = match &roster {
                    Some(r) => Some(tx.put_roster_snapshot(r, &me)?),
                    None => None,
                };
                tx.pin(
                    PinnedKind::GradeScale,
                    &omr_scoring::GradeScale::saudi(),
                    &me,
                )?;
                if let Some(o) = &def.outcomes {
                    tx.pin(PinnedKind::CloSet, &crate::outcomes::clo_set(def, o), &me)?;
                    tx.pin(PinnedKind::AttainmentPolicy, &o.policy.policy(), &me)?;
                }
                tx.create_batch(
                    &BatchRecord {
                        batch_id: BATCH_ID.into(),
                        exam_id: exam,
                        header: None,
                        campus: None,
                        operator: Some(me.id.clone()),
                        device_profile: None,
                        expected_count: None,
                        state: "open".into(),
                    },
                    &me,
                )?;
                Ok(roster)
            })
            .map_err(store_err)?;
        Ok(())
    }

    /// A roster given after grading ([`crate::ExamProject::set_roster`]): its snapshot (the
    /// roster in force its parent) is recorded by the next run's store stage, in one decision
    /// with the identity run that uses it, with `RosterReplaced`. A run that stops before its
    /// store stage records nothing; [`Self::unstage_roster`] drops it.
    pub fn stage_roster(&mut self, def: &ExamDefinition) {
        self.staged_roster = roster_snapshot_in(def, self.roster_snapshot.clone());
    }

    /// Drops a roster staged by [`Self::stage_roster`] (the run that was to record it failed).
    pub fn unstage_roster(&mut self) {
        self.staged_roster = None;
    }

    /// Stages every original (stage → fsync → link) in parallel; returns BLAKE3 addresses.
    pub fn stage_sources(
        &mut self,
        files: &[Option<SourceFile>],
        dup_of: &[Option<usize>],
    ) -> Result<Vec<Option<String>>, AppError> {
        let objects = self.project.objects();
        let staged: Vec<Result<Option<StagedObject>, AppError>> = files
            .par_iter()
            .zip(dup_of.par_iter())
            .map(|(f, d)| match (f, d) {
                (Some(f), None) => objects.stage_bytes(&f.bytes).map(Some).map_err(store_err),
                _ => Ok(None),
            })
            .collect();
        self.staged = staged.into_iter().collect::<Result<_, _>>()?;
        Ok(self
            .staged
            .iter()
            .map(|s| s.as_ref().map(|s| s.blake3.clone()))
            .collect())
    }

    /// Page rows for every page (bulk lane, ≤ 50 pages per transaction); duplicates and
    /// unreadable files become CAP-011 / CAP-001 exceptions. A page whose row the store holds
    /// already keeps it, also one spooled by an append that failed or was cancelled after its
    /// spool: page IDs derive from the content, so adding the same file again adds no row.
    pub fn spool_pages(
        &mut self,
        pages: &[(String, usize, u32, Option<usize>)],
        sources: &[SourceInfo],
        known: &std::collections::BTreeSet<String>,
    ) -> Result<(), AppError> {
        let staged = self.staged.clone();
        let engine = Actor::engine("ingest");
        let batch = self.batch_id.clone();
        // Pages already spooled keep their rows: an earlier batch's (`known`), and those of a
        // run that stopped after its spool (they keep that run's batch).
        let mut spooled = std::collections::BTreeSet::new();
        for s in staged.iter().flatten() {
            spooled.extend(
                self.project
                    .reader()
                    .pages_for_object(&s.blake3)
                    .map_err(store_err)?,
            );
        }
        let pages: Vec<&(String, usize, u32, Option<usize>)> = pages
            .iter()
            .filter(|p| !known.contains(&p.0) && !spooled.contains(&p.0))
            .collect();
        // Objects first.
        self.project
            .bulk(|tx| {
                for s in staged.iter().flatten() {
                    tx.insert_object(s, ObjectKind::Original)?;
                }
                Ok(())
            })
            .map_err(store_err)?;
        for (chunk_no, chunk) in pages
            .chunks(omr_store::project::BULK_GROUP_PAGES)
            .enumerate()
        {
            let base = chunk_no * omr_store::project::BULK_GROUP_PAGES;
            self.project
                .bulk(|tx| {
                    for (k, &&(ref page_id, src, _index, dup)) in chunk.iter().enumerate() {
                        match (staged.get(src).and_then(Option::as_ref), dup) {
                            (Some(obj), None) => {
                                tx.add_page(
                                    &PageRecord::spooled(page_id, &batch, (base + k) as i64, obj),
                                    &engine,
                                )?;
                            }
                            (_, Some(d)) => {
                                tx.raise_exception(
                                    &crate::ingest::cap001(page_id, "x")
                                        .clone()
                                        .with_detail(format!(
                                            "{} is byte-identical to {}",
                                            sources[src].name, sources[d].name
                                        ))
                                        .pipe_cap011(),
                                    &engine,
                                )?;
                            }
                            // Unreadable file: no original to spool; its CAP-001 is raised
                            // with the recognition results.
                            (None, None) => {}
                        }
                    }
                    Ok(())
                })
                .map_err(store_err)?;
        }
        Ok(())
    }

    /// Recognition, booklets, identity, exceptions and the score run; maps the store's
    /// exception rows onto the review items.
    pub fn write_results(
        &mut self,
        session: &mut Session,
        scoring: Option<&Scoring>,
        prior: Option<&Session>,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        let manifest = session.manifest.clone();
        let (recog_run, identity_run, batch_id) = (
            self.recog_run.clone(),
            self.identity_run.clone(),
            self.batch_id.clone(),
        );
        // An appended batch re-runs the exam (§13.4 "re-recognition creates a new run"): the
        // earlier run's open exceptions are superseded by the operator who added the batch;
        // the new run raises its own, and human resolutions carry over onto them.
        if prior.is_some() {
            let open: Vec<i64> = self
                .project
                .reader()
                .exceptions(Some("open"))
                .map_err(store_err)?
                .iter()
                .map(|e| e.exception_id)
                .collect();
            self.resolve(&open, &Reason::code("RERUN_ACCEPTED"), "superseded_by_run")?;
        }
        let published = self
            .project
            .reader()
            .key_versions(&exam)
            .map_err(store_err)?;
        let parent = self.parent_run.clone();
        self.project
            .decision(|tx| {
                tx.create_recognition_run(&RecognitionRunRecord {
                    run_id: recog_run.clone(),
                    exam_id: exam,
                    kind: RunKind::Final,
                    parent_run: parent.clone(),
                    engine_manifest: manifest,
                })?;
                if let Some(s) = scoring {
                    tx.put_form_map(&exam, &s.forms, None, "en", &me)?;
                    if !published.contains(&s.key.version) {
                        tx.put_key_version(&exam, &s.key, &me, None)?;
                    }
                    tx.put_policy_version(&exam, &s.policy, &me)?;
                }
                if let Some(o) = &session.definition.outcomes
                    && let Some(s) = scoring
                {
                    tx.put_mapping_set(
                        &crate::outcomes::mapping_set(
                            &session.definition,
                            o,
                            session.key.as_ref(),
                            s.forms.items.len(),
                        ),
                        &me,
                    )?;
                }
                Ok(())
            })
            .map_err(store_err)?;
        // Booklets: an appended batch renumbers the serials of sheets without a printed serial
        // (canonical rank), creates the new booklets and re-attaches only the pages that moved.
        let before: BTreeMap<&str, StoredBooklet> = prior
            .map(|p| {
                p.booklets
                    .iter()
                    .map(|b| {
                        let pages = b
                            .pages
                            .iter()
                            .enumerate()
                            .map(|(k, &pi)| (p.pages[pi].page_id.clone(), k as u16 + 1))
                            .collect();
                        (b.booklet_id.as_str(), (b.serial, pages))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if prior.is_some() {
            let kept: Vec<(String, u32)> = session
                .booklets
                .iter()
                .filter(|b| before.contains_key(b.booklet_id.as_str()))
                .map(|b| (b.booklet_id.clone(), b.serial))
                .collect();
            let top = session.booklets.iter().map(|b| b.serial).max().unwrap_or(0);
            self.project
                .decision(|tx| tx.renumber_booklets(&exam, &kept, top, &Actor::engine("collate")))
                .map_err(store_err)?;
        }
        for chunk in session
            .booklets
            .chunks(omr_store::project::BULK_GROUP_PAGES)
        {
            self.project
                .bulk(|tx| {
                    for b in chunk {
                        let known = before.get(b.booklet_id.as_str());
                        if known.is_none() {
                            tx.create_booklet(
                                &BookletRecord {
                                    booklet_id: b.booklet_id.clone(),
                                    exam_id: exam,
                                    serial: b.serial,
                                    form_code: b.form.map(|f| f as u16 + 1),
                                    bound_student: None,
                                    state: "collated".into(),
                                },
                                &Actor::engine("collate"),
                            )?;
                        }
                        for (k, &p) in b.pages.iter().enumerate() {
                            let page_id = &session.pages[p].page_id;
                            let page_no = k as u16 + 1;
                            if known.is_some_and(|(_, pages)| {
                                pages.iter().any(|(id, n)| id == page_id && *n == page_no)
                            }) {
                                continue;
                            }
                            tx.attach_page(
                                page_id,
                                &b.booklet_id,
                                page_no,
                                &Actor::engine("collate"),
                            )?;
                        }
                    }
                    Ok(())
                })
                .map_err(store_err)?;
        }
        // Page recognition (bulk lane) and refusals.
        let n_q = usize::from(session.definition.questions);
        let quarantined: std::collections::BTreeSet<&str> = prior
            .map(|p| {
                p.pages
                    .iter()
                    .filter(|pg| pg.status == PageStatus::Refused)
                    .map(|pg| pg.page_id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let mut refused_rows: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for chunk in session.pages.chunks(omr_store::project::BULK_GROUP_PAGES) {
            self.project
                .bulk(|tx| {
                    for p in chunk {
                        match (p.status, &p.read) {
                            (PageStatus::Read | PageStatus::Unattributed, Some(r)) => {
                                tx.record_page_recognition(
                                    &recog_run,
                                    &PageRecognition {
                                        page_id: p.page_id.clone(),
                                        registration: r.registration.clone(),
                                        questions: r
                                            .questions
                                            .iter()
                                            .filter(|q| usize::from(q.question) < n_q)
                                            .cloned()
                                            .collect(),
                                        exceptions: r.exceptions.clone(),
                                    },
                                )?;
                            }
                            // A superseded rescan keeps its reading and its link (COL-003,
                            // INFO, on the booklet graded instead).
                            (PageStatus::Duplicate, Some(r)) => {
                                tx.record_page_recognition(
                                    &recog_run,
                                    &PageRecognition {
                                        page_id: p.page_id.clone(),
                                        registration: r.registration.clone(),
                                        questions: r
                                            .questions
                                            .iter()
                                            .filter(|q| usize::from(q.question) < n_q)
                                            .cloned()
                                            .collect(),
                                        exceptions: r.exceptions.clone(),
                                    },
                                )?;
                                if let Some(e) = &p.refusal {
                                    tx.raise_exception(e, &Actor::engine("collate"))?;
                                }
                            }
                            // A blank back stays spooled (the original is kept) and its
                            // CAP-012 (INFO) is logged; nothing else refers to it.
                            (PageStatus::Blank, _) => {
                                if let Some(e) = &p.refusal {
                                    tx.raise_exception(e, &Actor::engine("ingest"))?;
                                }
                            }
                            (PageStatus::Refused, _) => {
                                if let Some(e) = &p.refusal {
                                    // Unreadable files have no original and no page row.
                                    if session.sources[p.source].blake3.is_some()
                                        && !quarantined.contains(p.page_id.as_str())
                                    {
                                        tx.set_page_state(
                                            &p.page_id,
                                            PageState::Quarantined,
                                            Some(&e.code),
                                            &Actor::engine("ingest"),
                                        )?;
                                    }
                                    let id = tx.raise_exception(e, &Actor::engine("ingest"))?;
                                    refused_rows.entry(p.page_id.clone()).or_default().push(id);
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })
                .map_err(store_err)?;
        }
        // Identity run and decisions; booklet, batch and exam exceptions.
        let mut raised: BTreeMap<(usize, String), Vec<i64>> = BTreeMap::new();
        let mut missing: BTreeMap<(usize, usize), Vec<i64>> = BTreeMap::new();
        let mut exam_rows: BTreeMap<(String, Option<String>), Vec<i64>> = BTreeMap::new();
        // The identity run names the roster it used: a roster given after grading is recorded
        // here, in the same decision (`RosterReplaced` holds counts and hashes, no student).
        let staged_roster = self.staged_roster.take();
        let in_force = self.roster_snapshot.clone();
        self.roster_snapshot = self
            .project
            .decision(|tx| {
                let roster = match &staged_roster {
                    Some(r) => {
                        let sha = tx.put_roster_snapshot(r, &me)?;
                        tx.append_event(&NewEvent::new(
                            "RosterReplaced",
                            "exam",
                            &exam.to_string(),
                            me.clone(),
                            json!({
                                "rows": r.rows.len(),
                                "sha384": sha,
                                "previous": r.parent,
                                "identity_mode": "roster",
                            }),
                        ))?;
                        Some(sha)
                    }
                    None => in_force,
                };
                tx.create_identity_run(&IdentityRunRecord {
                    run_id: identity_run.clone(),
                    exam_id: exam,
                    roster_snapshot: roster.clone(),
                    models: json!({"grid": "omr-identity"}),
                    thresholds: json!({}),
                })?;
                for (bi, b) in session.booklets.iter().enumerate() {
                    tx.put_identity_decision(
                        &IdentityDecisionRecord {
                            run_id: identity_run.clone(),
                            booklet_id: b.booklet_id.clone(),
                            student_id: b.identity.student.clone(),
                            status: store_status(b.identity.status),
                            logit_q: None,
                            margin_q: None,
                            evidence: json!({"candidates": b.identity.candidates.len()}),
                        },
                        &Actor::engine("identity"),
                        None,
                    )?;
                    for e in b.identity.exceptions.iter().chain(&b.exceptions) {
                        let id = tx.raise_exception(e, &Actor::engine("identity"))?;
                        raised.entry((bi, e.code.clone())).or_default().push(id);
                    }
                    // Questions of a missing page (COL-001) and human values a re-run
                    // contradicts (SYS-032): no page row holds them, so they are raised here.
                    for (q, qs) in b.questions.iter().enumerate() {
                        for e in qs.recognized.exceptions.iter().filter(|e| {
                            matches!(e.code.as_str(), "COL-001" | "SYS-032")
                                || e.sub_reason.as_deref() == Some("option_not_offered")
                        }) {
                            let id = tx.raise_exception(e, &Actor::engine("collate"))?;
                            missing.entry((bi, q)).or_default().push(id);
                        }
                    }
                }
                for e in session
                    .batch_exceptions
                    .iter()
                    .chain(&session.exam_exceptions)
                {
                    let id = tx.raise_exception(e, &Actor::engine("pipeline"))?;
                    exam_rows
                        .entry((e.code.clone(), e.sub_reason.clone()))
                        .or_default()
                        .push(id);
                }
                if let Some(Ok(run)) = scoring.map(|s| &s.run) {
                    tx.put_score_run(&exam, run, &me)?;
                }
                tx.close_batch(&batch_id, &me)?;
                Ok(roster)
            })
            .map_err(store_err)?;
        // Key sheets (§10.1) keep their reading in the run too (the confirmed key names their
        // scans), recorded after the students' rows so that those rows are numbered as in a
        // run without key sheets.
        let key_pages: Vec<&crate::model::PageState> = session
            .pages
            .iter()
            .filter(|p| p.status == PageStatus::KeySheet)
            .collect();
        for chunk in key_pages.chunks(omr_store::project::BULK_GROUP_PAGES) {
            self.project
                .bulk(|tx| {
                    for p in chunk {
                        match &p.read {
                            Some(r) => {
                                tx.record_page_recognition(
                                    &recog_run,
                                    &PageRecognition {
                                        page_id: p.page_id.clone(),
                                        registration: r.registration.clone(),
                                        questions: r
                                            .questions
                                            .iter()
                                            .filter(|q| usize::from(q.question) < n_q)
                                            .cloned()
                                            .collect(),
                                        exceptions: r.exceptions.clone(),
                                    },
                                )?;
                            }
                            // A key sheet that could not be read keeps its refusal.
                            None => {
                                if let Some(e) = &p.refusal {
                                    tx.raise_exception(e, &Actor::engine("ingest"))?;
                                }
                            }
                        }
                    }
                    Ok(())
                })
                .map_err(store_err)?;
        }
        // Map open exception rows to review items.
        let open = self
            .project
            .reader()
            .exceptions(Some("open"))
            .map_err(store_err)?;
        let mut by_page_q: PageQuestionRows = BTreeMap::new();
        for s in &open {
            if let Some(p) = &s.page_id {
                by_page_q
                    .entry((p.clone(), s.exception.target.question))
                    .or_default()
                    .push((
                        s.exception_id,
                        s.exception.code.clone(),
                        s.exception.sub_reason.clone(),
                    ));
            }
        }
        for it in &mut session.review.items {
            let page_id = it.page.map(|p| session.pages[p].page_id.clone());
            it.store_exceptions = match it.kind {
                ItemKind::Mark => page_id
                    .and_then(|p| by_page_q.get(&(p, it.question)))
                    .map(|v| v.iter().map(|x| x.0).collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .chain(
                        it.booklet
                            .zip(it.question)
                            .and_then(|(b, q)| missing.get(&(b, usize::from(q))))
                            .into_iter()
                            .flatten()
                            .copied(),
                    )
                    .collect(),
                ItemKind::Identity | ItemKind::Form => it
                    .booklet
                    .map(|b| {
                        raised
                            .iter()
                            .filter(|((bi, code), _)| {
                                *bi == b
                                    && match it.kind {
                                        ItemKind::Identity => {
                                            (code.starts_with("IDN") && code != "IDN-003")
                                                || code == "SYS-032"
                                        }
                                        _ => code == "IDN-003" || code == "SCO-005",
                                    }
                            })
                            .flat_map(|(_, ids)| ids.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                ItemKind::Notice => match (page_id, it.booklet) {
                    (Some(p), _) if refused_rows.contains_key(&p) => refused_rows[&p].clone(),
                    (Some(p), _) if it.code != "MRK-015" => by_page_q
                        .get(&(p, None))
                        .map(|v| {
                            v.iter()
                                .filter(|x| x.1 == it.code && x.2 == it.sub_reason)
                                .map(|x| x.0)
                                .collect()
                        })
                        .unwrap_or_default(),
                    (_, Some(b)) => raised
                        .get(&(b, it.code.clone()))
                        .cloned()
                        .unwrap_or_default(),
                    _ => exam_rows
                        .get(&(it.code.clone(), it.sub_reason.clone()))
                        .cloned()
                        .unwrap_or_default(),
                },
            };
        }
        // The key-sheet hold is lifted by the instructor's confirmation, not by a queue item.
        session.key_sheets.hold_rows = exam_rows
            .get(&(
                "SCO-003".to_string(),
                Some(crate::keysheet::HOLD.to_string()),
            ))
            .cloned()
            .unwrap_or_default();
        // Resolutions carried over from the earlier run resolve the new run's rows.
        if prior.is_some() {
            let carried: Vec<i64> = session
                .review
                .items
                .iter()
                .filter(|i| i.state == crate::review::ItemState::Resolved)
                .flat_map(|i| i.store_exceptions.iter().copied())
                .collect();
            self.resolve(&carried, &Reason::code("RERUN_ACCEPTED"), "carried_over")?;
        }
        self.checkpoint()?;
        Ok(())
    }

    /// The exam the package was created for.
    pub fn exam(&self) -> omr_model::ExamId {
        self.exam
    }

    /// Signs a checkpoint over the chain head with the device key (batch end, export, close,
    /// §14.4) and keeps it outside the package as this device's latest checkpoint of the
    /// project.
    pub fn checkpoint(&mut self) -> Result<(), AppError> {
        if let Some(cp) = self.project.batch_end().map_err(store_err)? {
            keys::record_checkpoint(&cp)?;
        }
        Ok(())
    }

    /// The latest checkpoint stored in the package, if any.
    pub fn latest_checkpoint(&self) -> Result<Option<omr_store::Checkpoint>, AppError> {
        Ok(self
            .project
            .reader()
            .checkpoints()
            .map_err(store_err)?
            .pop())
    }

    /// The latest snapshot binding of this project.
    pub fn snapshot_binding(&self) -> Result<Option<SnapshotBinding>, AppError> {
        snapshot_binding(&self.project.reader())
    }

    /// Records the snapshot just written ([`SNAPSHOT_EVENT`], decision lane): the hashes the
    /// next open must find. Returns the event's position.
    pub fn record_snapshot(
        &mut self,
        session_sha384: &str,
        pages_sha384: &str,
        index_sha384: &str,
        secret_sha384: Option<&str>,
    ) -> Result<i64, AppError> {
        let exam = self.exam.to_string();
        let mut payload = json!({
            "session_sha384": session_sha384,
            "pages_sha384": pages_sha384,
            "booklets_index_sha384": index_sha384,
        });
        if let Some(s) = secret_sha384 {
            payload["secret_sha384"] = json!(s);
        }
        self.project
            .decision(|tx| {
                tx.append_event(&NewEvent::new(
                    SNAPSHOT_EVENT,
                    "exam",
                    &exam,
                    Actor::system(),
                    payload,
                ))
                .map(|e| e.seq)
            })
            .map_err(store_err)
    }

    /// The reader of the open project (consistency checks).
    pub fn reader(&self) -> omr_store::Reader<'_> {
        self.project.reader()
    }

    /// The latest recognition run.
    pub fn recog_run(&self) -> &str {
        &self.recog_run
    }

    /// `ResponseCorrected` (+ `ExceptionResolved` for `resolves`).
    pub fn correction(
        &mut self,
        booklet_id: &str,
        position: u16,
        old: &Response,
        new: &Response,
        reason: &Reason,
        resolves: &[i64],
    ) -> Result<(), AppError> {
        let a = ReviewAction {
            booklet_id: booklet_id.into(),
            position,
            old: old.clone(),
            new: new.clone(),
            reason: reason.clone(),
            actor: self.actor.clone(),
            resolves: resolves.to_vec(),
        };
        self.project
            .decision(|tx| tx.record_review_action(&a).map(|_| ()))
            .map_err(store_err)
    }

    /// Resolves exception rows without a correction.
    pub fn resolve(
        &mut self,
        ids: &[i64],
        reason: &Reason,
        decision: &str,
    ) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let me = self.actor.clone();
        self.project
            .decision(|tx| tx.resolve_exceptions(ids, &me, reason.clone(), decision))
            .map_err(store_err)
    }

    /// Reopens exception rows (undo).
    pub fn reopen(&mut self, ids: &[i64]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let me = self.actor.clone();
        self.project
            .decision(|tx| tx.reopen_exceptions(ids, &me, Reason::code("CLERICAL_CORRECTION")))
            .map_err(store_err)
    }

    /// An identity confirmation by a reviewer.
    pub fn confirm_identity(
        &mut self,
        booklet_id: &str,
        student: Option<&str>,
        reason: &Reason,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let run = self.identity_run.clone();
        self.project
            .decision(|tx| {
                tx.put_identity_decision(
                    &IdentityDecisionRecord {
                        run_id: run.clone(),
                        booklet_id: booklet_id.into(),
                        student_id: student.map(str::to_string),
                        status: if student.is_some() {
                            IdentityStatus::Confirmed
                        } else {
                            IdentityStatus::NotOnRoster
                        },
                        logit_q: None,
                        margin_q: None,
                        evidence: json!({"source": "review"}),
                    },
                    &me,
                    Some(reason.clone()),
                )
            })
            .map_err(store_err)
    }

    /// The store form of a reviewer's reason (§14.4): the code, and a note's text kept in the
    /// store's `review_note` table under a random reference, the events citing only that
    /// reference and a salted SHA-384 (never guessable from a dictionary of names or student
    /// numbers; erasable without breaking the chain).
    pub fn reason(&mut self, code: &str, note: &str) -> Result<Reason, AppError> {
        if note.trim().is_empty() {
            return Ok(Reason::code(code));
        }
        let me = self.actor.clone();
        let text = note.trim().to_string();
        let r = self
            .project
            .decision(|tx| tx.put_note(&text, &me))
            .map_err(store_err)?;
        Ok(Reason::with_note(code, &r))
    }

    /// The compensating identity decision of an undone review action: the booklet's student
    /// and status as before the action, recorded by the reviewer who undid it (the projection
    /// and its log stay equal to the exam's state).
    pub fn restore_identity(
        &mut self,
        booklet_id: &str,
        student: Option<&str>,
        status: omr_identity::IdentityStatus,
        confirmed: bool,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let run = self.identity_run.clone();
        self.project
            .decision(|tx| {
                tx.put_identity_decision(
                    &IdentityDecisionRecord {
                        run_id: run.clone(),
                        booklet_id: booklet_id.into(),
                        student_id: student.map(str::to_string),
                        status: if confirmed {
                            if student.is_some() {
                                IdentityStatus::Confirmed
                            } else {
                                IdentityStatus::NotOnRoster
                            }
                        } else {
                            store_status(status)
                        },
                        logit_q: None,
                        margin_q: None,
                        evidence: json!({"source": "undo"}),
                    },
                    &me,
                    Some(Reason::code("CLERICAL_CORRECTION")),
                )
            })
            .map_err(store_err)
    }

    /// A review-queue event with a reviewer's reason (form choice, key changes).
    pub fn event_with_reason(
        &mut self,
        ty: &str,
        entity: &str,
        id: &str,
        payload: serde_json::Value,
        reason: &Reason,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        self.project
            .decision(|tx| {
                tx.append_event(
                    &NewEvent::new(ty, entity, id, me.clone(), payload).with_reason(reason.clone()),
                )
                .map(|_| ())
            })
            .map_err(store_err)
    }

    /// Stores the form map and policy version of a scoring state (identical ones are no-ops;
    /// the form map is immutable once stored, §14.3).
    pub fn put_scoring_setup(&mut self, s: &Scoring) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        self.project
            .decision(|tx| {
                tx.put_form_map(&exam, &s.forms, None, "en", &me)?;
                tx.put_policy_version(&exam, &s.policy, &me)?;
                Ok(())
            })
            .map_err(store_err)
    }

    /// Publishes a key version (`KeyVersionPublished`, payload hash only, §19.3).
    pub fn put_key_version(
        &mut self,
        key: &omr_scoring::KeyVersion,
        reason: &Reason,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        self.project
            .decision(|tx| {
                tx.put_key_version(&exam, key, &me, Some(reason.clone()))
                    .map(|_| ())
            })
            .map_err(store_err)
    }

    /// Publishes a key version with its provenance in the `KeyVersionPublished` event (a key
    /// confirmed from key sheets: source, scans, operator; §10.1).
    pub fn put_key_version_from(
        &mut self,
        key: &omr_scoring::KeyVersion,
        reason: Option<&Reason>,
        provenance: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        self.project
            .decision(|tx| {
                tx.put_key_version_from(&exam, key, &me, reason.cloned(), Some(provenance))
                    .map(|_| ())
            })
            .map_err(store_err)
    }

    /// Stores an outcome mapping set (§11.2), as a graded exam with a key does.
    pub fn put_mapping_set(&mut self, m: &omr_outcomes::MappingSet) -> Result<(), AppError> {
        let me = self.actor.clone();
        self.project
            .decision(|tx| tx.put_mapping_set(m, &me).map(|_| ()))
            .map_err(store_err)
    }

    /// A review-queue event (defer, escalate, rescan, form choice, undo marker).
    pub fn event(
        &mut self,
        ty: &str,
        entity: &str,
        id: &str,
        payload: serde_json::Value,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        self.project
            .decision(|tx| {
                tx.append_event(&NewEvent::new(ty, entity, id, me.clone(), payload))
                    .map(|_| ())
            })
            .map_err(store_err)
    }

    /// Stores a score run (idempotent by Run ID).
    pub fn put_score_run(&mut self, run: &omr_scoring::ScoreRun) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        self.project
            .decision(|tx| tx.put_score_run(&exam, run, &me).map(|_| ()))
            .map_err(store_err)
    }

    /// Records an export (`ExportCreated`).
    pub fn record_export(
        &mut self,
        purpose: &str,
        destination: &str,
        manifest_sha384: &str,
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let rec = ExportRecord {
            export_id: format!("export-{}", &manifest_sha384[..16]),
            purpose: purpose.into(),
            destination: destination.into(),
            recipient: None,
            pii: true,
            label: "restricted".into(),
            manifest_sha384: manifest_sha384.into(),
        };
        let exists = self
            .project
            .reader()
            .exports()
            .map_err(store_err)?
            .iter()
            .any(|x| x.export_id == rec.export_id);
        if exists {
            return Ok(());
        }
        self.project
            .decision(|tx| tx.record_export(&rec, &me).map(|_| ()))
            .map_err(store_err)
    }

    /// Head of the event chain (seq, hash).
    pub fn head(&self) -> Option<(i64, String)> {
        self.project
            .reader()
            .events(1, usize::MAX)
            .ok()
            .and_then(|v| v.last().map(|e| (e.seq, e.hash.clone())))
    }

    /// Every event of the chain, oldest first (audit views, tests).
    pub fn events(&self) -> Result<Vec<omr_store::EventRecord>, AppError> {
        self.project
            .reader()
            .events(1, usize::MAX)
            .map_err(store_err)
    }

    /// Exception rows of the project (`state`: `open`, `resolved`, or every row for `None`).
    pub fn exceptions(
        &self,
        state: Option<&str>,
    ) -> Result<Vec<omr_store::StoredException>, AppError> {
        self.project.reader().exceptions(state).map_err(store_err)
    }

    /// Number of events in the chain.
    pub fn event_count(&self) -> usize {
        self.project
            .reader()
            .events(1, usize::MAX)
            .map_or(0, |v| v.len())
    }

    /// Verifies the hash chain.
    pub fn verify(&self) -> Result<(), AppError> {
        self.project
            .verify_chain(None, &[])
            .map(|_| ())
            .map_err(store_err)
    }

    /// Flushes and closes the project with a signed checkpoint (kept outside the package too).
    pub fn close(self) -> Result<(), AppError> {
        if let Some(cp) = self.project.close().map_err(store_err)? {
            keys::record_checkpoint(&cp)?;
        }
        Ok(())
    }
}

/// The store form of a reviewer's reason without a store (in-memory runs): the code, and for
/// a note only a reference to it (its SHA-384 prefix). With a store,
/// [`StoreWriter::reason`] keeps the note in the store under a salted digest instead.
pub fn store_reason(code: &str, note: &str) -> Reason {
    if note.trim().is_empty() {
        Reason::code(code)
    } else {
        let h = omr_store::jcs::sha384_hex(note.trim().as_bytes());
        Reason::with_note(code, &format!("session-note:sha384:{}", &h[..32]))
    }
}

fn store_status(s: omr_identity::IdentityStatus) -> IdentityStatus {
    match s {
        omr_identity::IdentityStatus::AutoAccepted => IdentityStatus::AutoAccepted,
        omr_identity::IdentityStatus::NeedsConfirmation => IdentityStatus::NeedsConfirmation,
        omr_identity::IdentityStatus::Review => IdentityStatus::Review,
    }
}

/// CAP-011 rewrite of an ingest exception.
trait PipeCap011 {
    fn pipe_cap011(self) -> Exception;
}

impl PipeCap011 for Exception {
    fn pipe_cap011(self) -> Exception {
        Exception::must("CAP-011", omr_model::Severity::Info, omr_model::Scope::Page)
            .with_target(self.target)
            .with_detail(self.detail)
    }
}
