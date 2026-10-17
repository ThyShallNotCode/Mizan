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
    Action, Change, Decision, ItemKind, ItemState, Lane, ReviewItem, ReviewRecord,
};
use crate::score::Scoring;
use crate::store::{PAGES_FILE, SESSION_FILE, StoreWriter};
use omr_analytics::{AnalysisData, AnalyticsConfig, AnalyticsReport};
use omr_imaging::GrayImage;
use omr_model::{Response, Severity};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    sources: Mutex<BTreeMap<usize, Arc<SourceFile>>>,
    pages: Mutex<Vec<(usize, Arc<GrayImage>)>>,
    recognizer: Mutex<Option<Arc<Recognizer>>>,
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
    /// Grades inputs (see [`crate::pipeline::grade`]) and saves the session snapshot.
    pub fn grade(
        def: &ExamDefinition,
        inputs: &[PathBuf],
        opts: &GradeOptions,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<Self, AppError> {
        let g = crate::pipeline::grade(def, inputs, opts, progress)?;
        let p = Self::from_graded(g);
        p.save()?;
        Ok(p)
    }

    /// Wraps a pipeline result.
    pub fn from_graded(g: Graded) -> Self {
        Self {
            session: g.session,
            scoring: g.scoring,
            store: g.store,
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
        }
    }

    /// Reads the session snapshot (`cache/session.json` + `cache/pages.json`).
    fn read_session(project_dir: &Path) -> Result<Session, AppError> {
        let cache = project_dir.join("cache");
        let text = std::fs::read(cache.join(SESSION_FILE)).map_err(|e| {
            AppError::NotFound(format!(
                "{}: no graded exam here ({e}); run `omr grade` first",
                project_dir.display()
            ))
        })?;
        let mut session: Session = serde_json::from_slice(&text)
            .map_err(|e| AppError::Store(format!("session snapshot: {e}")))?;
        let pages = std::fs::read(cache.join(PAGES_FILE))
            .map_err(|e| AppError::Store(format!("page snapshot: {e}")))?;
        session.pages = serde_json::from_slice(&pages)
            .map_err(|e| AppError::Store(format!("page snapshot: {e}")))?;
        Ok(session)
    }

    /// Reopens a graded exam from its project package (session snapshot + store).
    pub fn open(project_dir: &Path, operator: &str) -> Result<Self, AppError> {
        let session = Self::read_session(project_dir)?;
        let store = StoreWriter::open(project_dir, &session.definition, operator)?;
        let scoring = Scoring::build(&session)?;
        Ok(Self {
            session,
            scoring,
            store: Some(store),
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
        })
    }

    /// Opens a graded exam read-only from its session snapshot (no store lock): dashboards and
    /// previews (crops need the store, so they are unavailable).
    pub fn open_read_only(project_dir: &Path) -> Result<Self, AppError> {
        let session = Self::read_session(project_dir)?;
        let scoring = Scoring::build(&session)?;
        Ok(Self {
            session,
            scoring,
            store: None,
            sources: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(Vec::new()),
            recognizer: Mutex::new(None),
        })
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

    /// Saves the session snapshot into the package's `cache/` (atomic renames). The page
    /// snapshot (bubble geometry for crops) is written once.
    pub fn save(&self) -> Result<(), AppError> {
        let Some(st) = &self.store else {
            return Ok(());
        };
        let dir = st.cache_dir();
        std::fs::create_dir_all(&dir)?;
        let write = |name: &str, bytes: Vec<u8>| -> Result<(), AppError> {
            let tmp = dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, dir.join(name))?;
            Ok(())
        };
        if !dir.join(PAGES_FILE).exists() {
            write(
                PAGES_FILE,
                serde_json::to_vec(&self.session.pages)
                    .map_err(|e| AppError::Internal(format!("page snapshot: {e}")))?,
            )?;
        }
        write(
            SESSION_FILE,
            serde_json::to_vec(&self.session)
                .map_err(|e| AppError::Internal(format!("session snapshot: {e}")))?,
        )
    }

    /// Closes the store.
    pub fn close(self) -> Result<(), AppError> {
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
    /// rescores incrementally.
    pub fn apply(&mut self, item_id: u64, action: Action) -> Result<ApplyOutcome, AppError> {
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
        match &action {
            Action::Resolve { decision, .. } => {
                changes.push(self.resolve_one(&item, decision)?);
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
                    changes.push(self.resolve_one(m, decision)?);
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
                    st.event(
                        ty,
                        "review_item",
                        &item_id.to_string(),
                        json!({"code": item.code}),
                    )?;
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
        });
        self.save()?;
        Ok(ApplyOutcome {
            next: self.next_after(lane, item_id),
            affected,
        })
    }

    fn resolve_one(&mut self, item: &ReviewItem, decision: &Decision) -> Result<Change, AppError> {
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
                            reason_for(&item.code),
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
                    st.confirm_identity(&booklet_id, student.as_deref())?;
                    st.resolve(&item.store_exceptions, "ID_CORRECTION", "student")?;
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
                    st.event(
                        "BookletFormConfirmed",
                        "booklet",
                        &booklet_id,
                        json!({"form": f as u64 + 1}),
                    )?;
                    st.resolve(&item.store_exceptions, "CLERICAL_CORRECTION", "form")?;
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
                    let reason = if item.lane == Lane::Capture {
                        "NOT_EXAM_MATERIAL"
                    } else {
                        "ADJUDICATION"
                    };
                    if item.store_exceptions.is_empty() {
                        st.event(
                            "ReviewItemAcknowledged",
                            "review_item",
                            &item.id.to_string(),
                            json!({"code": item.code}),
                        )?;
                    } else {
                        st.resolve(&item.store_exceptions, reason, "acknowledge")?;
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
        let Some(pos) = self.session.review.history.iter().rposition(|r| !r.undone) else {
            return Ok(None);
        };
        let rec = self.session.review.history[pos].clone();
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
                        "CLERICAL_CORRECTION",
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
        self.save()?;
        Ok(Some(rec.item))
    }

    /// Sets the key of one question of one form — `form` is the 0-based index into the
    /// definition's forms, `question` the **1-based** printed question (as in the mapping
    /// editor and the shell); `answer`: "", "B", "A|C" or "A,C", "*" — a new key version,
    /// rescored without re-projecting (L2, A4). With identical forms (shared master items)
    /// every form's copy of the key changes together.
    pub fn set_key(&mut self, form: usize, question: u32, answer: &str) -> Result<(), AppError> {
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
            return Err(AppError::Scoring("the exam has no key yet".into()));
        };
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
        if let Some(st) = self.store.as_mut() {
            st.event(
                "KeyEntryChanged",
                "exam",
                &def.exam_id.to_string(),
                json!({"form": form as u64 + 1, "question": question, "key_version": key.version}),
            )?;
        }
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

    /// Writes every export to `dir` (see [`crate::export`]).
    pub fn export(&mut self, dir: &Path) -> Result<crate::export::ExportSummary, AppError> {
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
        Ok(summary)
    }
}
