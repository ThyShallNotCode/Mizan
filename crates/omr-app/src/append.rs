//! A later batch appended to a graded exam (a second section scanned on another day, §8.1,
//! §13.4): the exam is graded again over every original (the stored ones and the new files),
//! so batch statistics, collation, serials and the queue are exactly those of grading
//! everything at once; the key in force and every human resolution carry over by stable key
//! (booklet = its first page, question, code), and a re-run that contradicts a human value
//! raises SYS-032 (`rerun_disagrees_with_override`) instead of overwriting it.

use crate::AppError;
use crate::ingest::SourceFile;
use crate::model::{Booklet, EffectiveKey, FormSource, ResponseStatus, Session};
use crate::review::{ItemKind, ItemState, ReviewItem};
use crate::store::StoreWriter;
use omr_model::exceptions::Target;
use omr_model::{Exception, Scope, Severity};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What an appended batch did.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AppendReport {
    /// The new scan batch, e.g. `batch-2`.
    pub batch: String,
    /// Input files read for this batch.
    pub files: usize,
    /// Pages the batch added (new originals).
    pub new_pages: usize,
    /// Pages of this batch that were byte-identical to an ingested file (CAP-011).
    pub duplicates: usize,
    /// Booklets before and after.
    pub booklets_before: usize,
    /// Booklets after the batch.
    pub booklets_after: usize,
    /// Review resolutions carried over from the earlier run.
    pub carried_over: usize,
    /// Human values the re-run contradicts (SYS-032, queued).
    pub conflicts: usize,
}

/// An input file read for grading, or the path of an unreadable one and why.
pub type SourceRead = Result<SourceFile, (PathBuf, String)>;

/// The originals of an exam, read back from the project store (verified), as the sources of
/// a re-run. Unreadable inputs, which have no stored original, come back as the same refusal.
pub fn stored_sources(session: &Session, store: &StoreWriter) -> Result<Vec<SourceRead>, AppError> {
    let by_sha: BTreeMap<&str, &str> = session
        .sources
        .iter()
        .filter_map(|s| s.blake3.as_deref().map(|b| (s.sha384.as_str(), b)))
        .collect();
    let mut out = Vec::with_capacity(session.sources.len());
    for s in &session.sources {
        match by_sha.get(s.sha384.as_str()) {
            Some(b3) => {
                let bytes = store.read_object(b3, &s.sha384)?;
                let f = SourceFile::from_bytes(Path::new(&s.name), bytes).ok_or_else(|| {
                    AppError::Internal(format!("{}: stored original of unknown type", s.name))
                })?;
                out.push(Ok(f));
            }
            None => {
                // `name (unreadable: why)`.
                let (name, why) = s
                    .name
                    .split_once(" (unreadable: ")
                    .map_or((s.name.as_str(), "file could not be read"), |(n, w)| {
                        (n, w.trim_end_matches(')'))
                    });
                out.push(Err((PathBuf::from(name), why.to_string())));
            }
        }
    }
    Ok(out)
}

/// The key of the re-run: a supplied (or corrected) key stays in force with its version; an
/// inferred key is inferred again over every booklet, as a new version when it changed.
pub(crate) fn carry_key(prior: &Session, key: &mut Option<EffectiveKey>) {
    match (&prior.key, key.as_mut()) {
        (Some(old), _) if !old.inferred => *key = Some(old.clone()),
        (Some(old), Some(new)) if new.inferred => {
            new.version = if new.forms == old.forms && new.points == old.points {
                old.version
            } else {
                old.version + 1
            };
        }
        _ => {}
    }
}

fn first_page(s: &Session, b: &Booklet) -> String {
    b.pages
        .first()
        .map(|&p| s.pages[p].page_id.clone())
        .unwrap_or_default()
}

/// SYS-032 (ERROR): a re-run disagrees with a human resolution, which is kept (§13.4).
fn sys032(scope: Scope, serial: u32, question: Option<u16>, detail: String) -> Exception {
    Exception::must("SYS-032", Severity::Error, scope)
        .with_sub_reason("rerun_disagrees_with_override")
        .with_target(Target {
            booklet: Some(serial),
            question,
            ..Target::default()
        })
        .with_detail(detail)
}

/// The re-run's booklet of every page ID: a page's own booklet, or for a copy the re-run
/// superseded as a rescan (COL-003, e.g. a cleaner scan of the sheet in the new batch) the
/// booklet of the copy graded instead.
fn booklet_by_page(s: &Session) -> BTreeMap<String, usize> {
    let by_serial: BTreeMap<u32, usize> = s
        .booklets
        .iter()
        .enumerate()
        .map(|(i, b)| (b.serial, i))
        .collect();
    s.pages
        .iter()
        .filter_map(|p| {
            let bi = p.booklet.or_else(|| {
                let e = p.refusal.as_ref().filter(|e| e.code == "COL-003")?;
                by_serial.get(&e.target.booklet?).copied()
            })?;
            Some((p.page_id.clone(), bi))
        })
        .collect()
}

/// Before the queue is built: human values the re-run contradicts are kept and flagged
/// (SYS-032), so they are queued instead of silently replaced. A booklet is found by its first
/// page, also when that page is now a superseded rescan.
pub(crate) fn mark_conflicts(prior: &Session, session: &mut Session) -> usize {
    let by_page = booklet_by_page(session);
    let mut conflicts = 0;
    for ob in &prior.booklets {
        let Some(&bi) = by_page.get(&first_page(prior, ob)) else {
            continue;
        };
        let b = &mut session.booklets[bi];
        for (q, oq) in ob.questions.iter().enumerate() {
            let Some(nq) = b.questions.get_mut(q) else {
                continue;
            };
            if matches!(
                oq.status,
                ResponseStatus::Confirmed | ResponseStatus::Changed
            ) && nq.status == ResponseStatus::Auto
                && nq.current != oq.current
            {
                nq.recognized.exceptions.push(sys032(
                    Scope::Item,
                    b.serial,
                    Some(q as u16),
                    "the re-run reads this answer differently from a reviewer's decision; the \
                     reviewer's answer is kept until someone confirms either"
                        .into(),
                ));
                nq.current = oq.current.clone();
                nq.status = ResponseStatus::Pending;
                conflicts += 1;
            }
        }
        let auto_identity = !b
            .identity
            .exceptions
            .iter()
            .any(|e| e.severity >= Severity::Error);
        if ob.identity.confirmed && auto_identity && b.identity.student != ob.identity.student {
            b.identity.exceptions.push(sys032(
                Scope::Booklet,
                b.serial,
                None,
                "the re-run assigns this booklet to another student than a reviewer did; the \
                 reviewer's choice is kept until someone confirms either"
                    .into(),
            ));
            b.identity.student = ob.identity.student.clone();
            conflicts += 1;
        }
        // A reviewer's form stands (the engine's choice of a declared form never overrides).
        if ob.form_source == FormSource::Reviewed && b.form != ob.form {
            let has_item = b
                .exceptions
                .iter()
                .any(|e| e.code == "IDN-003" || e.code == "SCO-005");
            if !has_item {
                b.form = ob.form;
                b.form_source = FormSource::Reviewed;
            }
        }
    }
    conflicts
}

/// The stable key of a review item: kind, code, sub-reason, question and subject (a
/// booklet's first page, a page, or the exam).
fn item_key(s: &Session, it: &ReviewItem) -> (String, String, String, Option<u16>, String) {
    let subject = match (it.kind, it.booklet, it.page) {
        (ItemKind::Notice, _, Some(p)) => s.pages[p].page_id.clone(),
        (_, Some(b), _) => format!("booklet:{}", first_page(s, &s.booklets[b])),
        (_, None, Some(p)) => s.pages[p].page_id.clone(),
        _ => String::new(),
    };
    (
        format!("{:?}", it.kind),
        it.code.clone(),
        it.sub_reason.clone().unwrap_or_default(),
        it.question,
        subject,
    )
}

/// After the queue is built: every item the earlier run had deferred, escalated, sent to
/// rescan or resolved gets that state again, and a resolution's booklet data (answer,
/// student, form, released hold) is applied. Returns the carried resolutions and the map of
/// old → new item IDs.
pub(crate) fn carry_review(prior: &Session, session: &mut Session) -> (usize, BTreeMap<u64, u64>) {
    let old: BTreeMap<_, &ReviewItem> = prior
        .review
        .items
        .iter()
        .map(|it| (item_key(prior, it), it))
        .collect();
    let new_keys: Vec<_> = session
        .review
        .items
        .iter()
        .map(|it| item_key(session, it))
        .collect();
    let mut ids = BTreeMap::new();
    let mut carried = 0;
    for (k, key) in new_keys.into_iter().enumerate() {
        let Some(o) = old.get(&key) else {
            continue;
        };
        ids.insert(o.id, session.review.items[k].id);
        if o.state == ItemState::Open {
            continue;
        }
        let it = &mut session.review.items[k];
        it.state = o.state;
        it.resolution = o.resolution.clone();
        it.deferrals = o.deferrals;
        if o.state != ItemState::Resolved {
            continue;
        }
        carried += 1;
        let (Some(ob), Some(bi)) = (o.booklet.map(|b| &prior.booklets[b]), it.booklet) else {
            continue;
        };
        let (kind, q, code) = (it.kind, it.question, it.code.clone());
        let b = &mut session.booklets[bi];
        match kind {
            ItemKind::Mark => {
                let q = usize::from(q.unwrap_or(0));
                if let (Some(nq), Some(oq)) = (b.questions.get_mut(q), ob.questions.get(q)) {
                    nq.current = oq.current.clone();
                    nq.status = oq.status;
                }
            }
            ItemKind::Identity => {
                b.identity.student = ob.identity.student.clone();
                b.identity.confirmed = ob.identity.confirmed;
            }
            ItemKind::Form => {
                b.form = ob.form;
                b.form_source = ob.form_source;
            }
            ItemKind::Notice => {
                if b.hold.as_deref() == Some(code.as_str()) && ob.hold.is_none() {
                    b.hold = None;
                }
            }
        }
    }
    // Probable mis-keys (SCO-001, §12.2) are judged on the score run, not built with the
    // queue: each one carries over with its state and decision (a key kept with a note stays
    // kept), and the next judgement of the flags re-opens, withdraws or adds as the re-run's
    // item analysis says.
    for o in prior
        .review
        .items
        .iter()
        .filter(|i| i.code == "SCO-001" && i.flag.is_some())
    {
        let mut it = o.clone();
        it.id = session.review.items.len() as u64 + 1;
        ids.insert(o.id, it.id);
        if it.state == ItemState::Resolved && it.resolution.is_some() {
            carried += 1;
        }
        session.review.items.push(it);
    }
    (carried, ids)
}
