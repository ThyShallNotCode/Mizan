//! The project store side of the pipeline (§14): the exam package, spool-first originals,
//! recognition, identity, exceptions, score runs and review events, all through `omr-store`
//! (hash-chained events). The writer is used from the scheduler thread only (one writer, §4.5).

use crate::AppError;
use crate::exam::ExamDefinition;
use crate::ingest::SourceFile;
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

/// Open exception rows by (page ID, question): (row ID, code, sub-reason).
type PageQuestionRows = BTreeMap<(String, Option<u16>), Vec<(i64, String, Option<String>)>>;

fn store_err(e: omr_store::StoreError) -> AppError {
    AppError::Store(e.to_string())
}

/// The open project and what the pipeline staged into it.
pub struct StoreWriter {
    project: Project,
    exam: omr_model::ExamId,
    actor: Actor,
    staged: Vec<Option<StagedObject>>,
}

impl std::fmt::Debug for StoreWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StoreWriter({:?})", self.project.paths().root)
    }
}

fn open_options() -> OpenOptions {
    OpenOptions {
        device_id: "device:omr-app".into(),
        clock: Box::new(SystemClock),
        // Location checks (SYS-015) refuse sync folders and network mounts; scratch and test
        // folders are local, so the check stays on.
        check_location: false,
        ..OpenOptions::default()
    }
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
            open_options(),
        )
        .map_err(store_err)?;
        Ok(Self {
            project,
            exam: def.exam_id,
            actor: Actor::user(operator, "Instructor"),
            staged: Vec::new(),
        })
    }

    /// Opens an existing package.
    pub fn open(dir: &Path, def: &ExamDefinition, operator: &str) -> Result<Self, AppError> {
        let project = Project::open(dir, open_options()).map_err(store_err)?;
        Ok(Self {
            project,
            exam: def.exam_id,
            actor: Actor::user(operator, "Instructor"),
            staged: Vec::new(),
        })
    }

    /// Package root.
    pub fn root(&self) -> PathBuf {
        self.project.paths().root.clone()
    }

    /// The `cache/` folder (session snapshot).
    pub fn cache_dir(&self) -> PathBuf {
        self.project.paths().cache.clone()
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
        let roster = def.roster.as_ref().map(|rows| RosterSnapshotIn {
            parent: None,
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
        });
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
        self.project
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
                if let Some(r) = &roster {
                    tx.put_roster_snapshot(r, &me)?;
                }
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
                Ok(())
            })
            .map_err(store_err)
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
    /// unreadable files become CAP-011 / CAP-001 exceptions.
    pub fn spool_pages(
        &mut self,
        pages: &[(String, usize, u32, Option<usize>)],
        sources: &[SourceInfo],
    ) -> Result<(), AppError> {
        let staged = self.staged.clone();
        let engine = Actor::engine("ingest");
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
                    for (k, (page_id, src, _index, dup)) in chunk.iter().enumerate() {
                        match (staged.get(*src).and_then(Option::as_ref), dup) {
                            (Some(obj), None) => {
                                tx.add_page(
                                    &PageRecord::spooled(page_id, BATCH_ID, (base + k) as i64, obj),
                                    &engine,
                                )?;
                            }
                            (_, Some(d)) => {
                                tx.raise_exception(
                                    &crate::ingest::cap001(page_id, "x")
                                        .clone()
                                        .with_detail(format!(
                                            "{} is byte-identical to {}",
                                            sources[*src].name, sources[*d].name
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
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        let exam = self.exam;
        let manifest = session.manifest.clone();
        self.project
            .decision(|tx| {
                tx.create_recognition_run(&RecognitionRunRecord {
                    run_id: RECOG_RUN.into(),
                    exam_id: exam,
                    kind: RunKind::Final,
                    parent_run: None,
                    engine_manifest: manifest,
                })?;
                if let Some(s) = scoring {
                    tx.put_form_map(&exam, &s.forms, None, "en", &me)?;
                    tx.put_key_version(&exam, &s.key, &me, None)?;
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
        // Booklets.
        for chunk in session
            .booklets
            .chunks(omr_store::project::BULK_GROUP_PAGES)
        {
            self.project
                .bulk(|tx| {
                    for b in chunk {
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
                        for (k, &p) in b.pages.iter().enumerate() {
                            tx.attach_page(
                                &session.pages[p].page_id,
                                &b.booklet_id,
                                k as u16 + 1,
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
        let mut refused_rows: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for chunk in session.pages.chunks(omr_store::project::BULK_GROUP_PAGES) {
            self.project
                .bulk(|tx| {
                    for p in chunk {
                        match (p.status, &p.read) {
                            (PageStatus::Read | PageStatus::Unattributed, Some(r)) => {
                                tx.record_page_recognition(
                                    RECOG_RUN,
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
                            (PageStatus::Refused, _) => {
                                if let Some(e) = &p.refusal {
                                    // Unreadable files have no original and no page row.
                                    if session.sources[p.source].blake3.is_some() {
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
        let mut exam_rows: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        self.project
            .decision(|tx| {
                tx.create_identity_run(&IdentityRunRecord {
                    run_id: IDENTITY_RUN.into(),
                    exam_id: exam,
                    roster_snapshot: None,
                    models: json!({"grid": "omr-identity"}),
                    thresholds: json!({}),
                })?;
                for (bi, b) in session.booklets.iter().enumerate() {
                    tx.put_identity_decision(
                        &IdentityDecisionRecord {
                            run_id: IDENTITY_RUN.into(),
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
                    // Questions of a missing page (COL-001): no page row holds them, so they
                    // are raised here (pending, never blank).
                    for (q, qs) in b.questions.iter().enumerate() {
                        for e in qs
                            .recognized
                            .exceptions
                            .iter()
                            .filter(|e| e.code == "COL-001")
                        {
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
                    exam_rows.entry(e.code.clone()).or_default().push(id);
                }
                if let Some(Ok(run)) = scoring.map(|s| &s.run) {
                    tx.put_score_run(&exam, run, &me)?;
                }
                tx.close_batch(BATCH_ID, &me)?;
                Ok(())
            })
            .map_err(store_err)?;
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
                                            code.starts_with("IDN") && code != "IDN-003"
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
                    _ => exam_rows.get(&it.code).cloned().unwrap_or_default(),
                },
            };
        }
        self.project.batch_end().map_err(store_err)?;
        Ok(())
    }

    /// `ResponseCorrected` (+ `ExceptionResolved` for `resolves`).
    pub fn correction(
        &mut self,
        booklet_id: &str,
        position: u16,
        old: &Response,
        new: &Response,
        reason: &str,
        resolves: &[i64],
    ) -> Result<(), AppError> {
        let a = ReviewAction {
            booklet_id: booklet_id.into(),
            position,
            old: old.clone(),
            new: new.clone(),
            reason: Reason::code(reason),
            actor: self.actor.clone(),
            resolves: resolves.to_vec(),
        };
        self.project
            .decision(|tx| tx.record_review_action(&a).map(|_| ()))
            .map_err(store_err)
    }

    /// Resolves exception rows without a correction.
    pub fn resolve(&mut self, ids: &[i64], reason: &str, decision: &str) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let me = self.actor.clone();
        self.project
            .decision(|tx| tx.resolve_exceptions(ids, &me, Reason::code(reason), decision))
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
    ) -> Result<(), AppError> {
        let me = self.actor.clone();
        self.project
            .decision(|tx| {
                tx.put_identity_decision(
                    &IdentityDecisionRecord {
                        run_id: IDENTITY_RUN.into(),
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
                    Some(Reason::code("ID_CORRECTION")),
                )
            })
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

    /// Flushes and closes the project.
    pub fn close(self) -> Result<(), AppError> {
        self.project.close().map(|_| ()).map_err(store_err)
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
