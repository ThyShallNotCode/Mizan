//! The session snapshot checked before it is trusted (§14.4: the snapshot is a cache, the
//! store and its event chain are the record).
//!
//! Three layers, cheapest first:
//! - [`validate`]: every index of the snapshot points inside it, so a damaged snapshot is
//!   SYS-010 with remediation, never a panic;
//! - the snapshot's SHA-384 against its `SnapshotSaved` event (in [`crate::app`]);
//! - [`against_store`]: what decides a grade, replayed from the store's decision tables
//!   (themselves replayed against their events on every open, `Reader::verify_projections`):
//!   every recognised response against the recognition run, every reviewed response against
//!   its latest correction, every booklet's student against its latest identity decision and
//!   the key in force against its published key version. A session edited behind the app's
//!   back is SYS-011 even when its hash was forged too.

use crate::AppError;
use crate::model::{ResponseStatus, Session};
use omr_model::Response;
use std::collections::BTreeMap;

/// The refusal of a snapshot that is not what the app saved (R10: what happened, what to do).
pub fn tampered(what: &str) -> AppError {
    AppError::Store(format!(
        "SYS-011 the exam snapshot does not match the audit log: {what}. It was changed outside \
         the app (or is damaged), so the exam is not opened. Restore the project folder from \
         your backup or the last signed export, and report it to the exam's QA officer"
    ))
}

/// The refusal of a snapshot that cannot be used (SYS-010).
pub fn damaged(what: &str) -> AppError {
    AppError::Store(format!(
        "SYS-010 the exam snapshot is damaged: {what}. Restore the project folder from your \
         backup (the audit log in project.db is intact)"
    ))
}

/// Every index of the snapshot points inside it (SEC-10): items to booklets and pages,
/// booklets to pages and forms, pages to sources and booklets, the key to the forms and
/// questions.
pub fn validate(s: &Session) -> Result<(), AppError> {
    let def = &s.definition;
    let (nb, np, ns) = (s.booklets.len(), s.pages.len(), s.sources.len());
    let nq = usize::from(def.questions);
    let nf = def.forms.len();
    for p in &s.pages {
        if p.source >= ns || p.booklet.is_some_and(|b| b >= nb) {
            return Err(damaged(&format!(
                "page {} points outside the exam",
                p.page_id
            )));
        }
    }
    for b in &s.booklets {
        let bad = b.pages.iter().any(|&p| p >= np)
            || b.questions.len() != nq
            || b.questions.iter().any(|q| q.page >= np)
            || b.form.is_some_and(|f| f >= nf)
            || b.declared_form.is_some_and(|f| f >= nf);
        if bad {
            return Err(damaged(&format!("{} points outside the exam", b.label())));
        }
    }
    for it in &s.review.items {
        if it.booklet.is_some_and(|b| b >= nb)
            || it.page.is_some_and(|p| p >= np)
            || it.question.is_some_and(|q| usize::from(q) >= nq)
            || it.crop.as_ref().is_some_and(|c| c.page >= np)
        {
            return Err(damaged(&format!(
                "review item {} points outside the exam",
                it.id
            )));
        }
    }
    if let Some(k) = &s.key
        && (k.forms.len() != nf || k.forms.iter().any(|f| f.len() != nq))
    {
        return Err(damaged(
            "the key does not fit the exam's forms and questions",
        ));
    }
    Ok(())
}

/// The snapshot against the store (see the module documentation). Decisions recorded after
/// the snapshot's own `SnapshotSaved` event (`upto`) are left out: a crash between a review
/// action and the snapshot save leaves the session one action behind, which is not tampering.
pub fn against_store(
    s: &Session,
    reader: &omr_store::Reader<'_>,
    exam: omr_model::ExamId,
    recog_run: Option<&str>,
    upto: Option<i64>,
) -> Result<(), AppError> {
    let store = |e: omr_store::StoreError| AppError::Store(e.to_string());
    let within = |seq: i64| upto.is_none_or(|u| seq <= u);
    if s.definition.exam_id != exam {
        return Err(tampered("it names another exam than the project"));
    }
    // Recognition: what the engine read is what the run recorded.
    let read = match recog_run {
        Some(r) => reader.run_question_results(r).map_err(store)?,
        None => BTreeMap::new(),
    };
    // Reviewed responses: the latest correction per (booklet, question).
    let mut corrected: BTreeMap<(&str, u16), Response> = BTreeMap::new();
    let corrections = reader.all_corrections().map_err(store)?;
    for c in &corrections {
        if within(c.event_seq) {
            corrected.insert((c.booklet_id.as_str(), c.position), c.new.clone());
        }
    }
    for b in &s.booklets {
        for (q, qs) in b.questions.iter().enumerate() {
            let page = &s.pages[qs.page].page_id;
            if let Some(r) = read.get(&(page.clone(), q as u16))
                && r.response != qs.recognized.response
            {
                return Err(tampered(&format!(
                    "{} question {}: the recognised answer differs from the recognition run",
                    b.label(),
                    q + 1
                )));
            }
            let ok = match corrected.get(&(b.booklet_id.as_str(), q as u16)) {
                Some(new) => *new == qs.current,
                None => {
                    matches!(qs.status, ResponseStatus::Auto | ResponseStatus::Pending)
                        && qs.current == qs.recognized.response
                }
            };
            if !ok {
                return Err(tampered(&format!(
                    "{} question {}: the answer differs from the audited review decisions",
                    b.label(),
                    q + 1
                )));
            }
        }
    }
    // Identity: the latest decision logged for each booklet.
    let mut student: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (booklet, seq, st) in reader.identity_log().map_err(store)? {
        if within(seq) {
            student.insert(booklet, st);
        }
    }
    for b in &s.booklets {
        if let Some(st) = student.get(&b.booklet_id)
            && *st != b.identity.student
        {
            return Err(tampered(&format!(
                "{}: the student differs from the audited identity decisions",
                b.label()
            )));
        }
    }
    // The key in force is a published key version.
    if let Some(k) = &s.key {
        let (kv, _) = crate::score::key_version(&s.definition, k);
        let stored = reader
            .key_version(&exam, k.version)
            .map_err(|_| tampered(&format!("key version {} was never published", k.version)))?;
        if stored.entries != kv.entries {
            return Err(tampered(&format!(
                "the key differs from published key version {}",
                k.version
            )));
        }
    }
    Ok(())
}
