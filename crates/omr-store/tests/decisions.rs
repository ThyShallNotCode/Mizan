//! Decision integrity (§14.2–§14.5, Appendix A): superseded identity decisions stay in the
//! log and every commitment opens against its event; engines never perform decision actions;
//! only Appendix A exceptions are stored; CAP-011 needs an equal SHA-384; one policy number
//! names one body; a failed automatic checkpoint never reports a committed command as unsaved.

mod common;

use common::*;
use omr_model::{Exception, Response, Scope, Severity};
use omr_scoring::PolicyVersion;
use omr_store::{
    Actor, BatchRecord, BookletRecord, CheckpointSigner, IdentityDecisionRecord, IdentityRunRecord,
    IdentityStatus, NewEvent, Project, Reason, ReviewAction, StoreError,
};
use rusqlite::Connection;
use serde_json::{Value, json};

fn reviewer() -> Actor {
    Actor::user("principal:reviewer", "Reviewer")
}

fn decision(student: &str, status: IdentityStatus) -> IdentityDecisionRecord {
    IdentityDecisionRecord {
        run_id: "ir-1".into(),
        booklet_id: "bk-1".into(),
        student_id: Some(student.into()),
        status,
        logit_q: Some(4000),
        margin_q: Some(900),
        evidence: json!({"channels": ["grid"]}),
    }
}

/// A project with one booklet and one identity run.
fn with_booklet(root: &std::path::Path) -> Project {
    let mut p = project_with_exam(root);
    p.decision(|tx| {
        tx.create_booklet(
            &BookletRecord {
                booklet_id: "bk-1".into(),
                exam_id: exam_id(),
                serial: 1,
                form_code: Some(1),
                bound_student: None,
                state: "collated".into(),
            },
            &instructor(),
        )?;
        tx.create_identity_run(&IdentityRunRecord {
            run_id: "ir-1".into(),
            exam_id: exam_id(),
            roster_snapshot: None,
            models: json!({"grid": "v1"}),
            thresholds: json!({"auto_accept_q": 7066}),
        })
    })
    .unwrap();
    p
}

/// `SHA-384(nonce ‖ JCS(value))`, hex: the event commitment of a student-linked value.
fn commitment(nonce_hex: &str, value: &Value) -> String {
    let mut buf = hex::decode(nonce_hex).unwrap();
    buf.extend_from_slice(omr_store::jcs::value_to_jcs(value).unwrap().as_bytes());
    omr_store::jcs::sha384_hex(&buf)
}

#[test]
fn superseded_identity_decisions_are_kept_and_their_commitments_open() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    {
        let mut p = with_booklet(&root);
        p.decision(|tx| {
            tx.put_identity_decision(
                &decision("pid-a", IdentityStatus::NeedsConfirmation),
                &Actor::engine("identity"),
                None,
            )
        })
        .unwrap();
        // A human then assigns the booklet to another student.
        p.decision(|tx| {
            tx.put_identity_decision(
                &decision("pid-b", IdentityStatus::Confirmed),
                &reviewer(),
                Some(Reason::code("ID_CORRECTION")),
            )
        })
        .unwrap();
        p.close().unwrap();
    }
    let p = Project::open(&root, opts()).unwrap();
    let r = p.reader();
    // The projection holds the latest decision only…
    let cur = r.identity_decisions("ir-1").unwrap();
    assert_eq!(cur.len(), 1);
    assert_eq!(cur[0].student_id.as_deref(), Some("pid-b"));
    // …and the log holds both, in event order, each opening its event's commitment.
    let hist = r.identity_history("ir-1", "bk-1").unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(
        (hist[0].student_id.as_deref(), hist[0].status),
        (Some("pid-a"), IdentityStatus::NeedsConfirmation)
    );
    assert_eq!(hist[0].actor_id, "engine:identity");
    assert_eq!(
        (hist[1].student_id.as_deref(), hist[1].status),
        (Some("pid-b"), IdentityStatus::Confirmed)
    );
    for h in &hist {
        let ev = &r.events(h.event_seq, 1).unwrap()[0];
        let target = json!({"student_id": h.student_id, "status": h.status.as_str()});
        assert_eq!(
            ev.payload["target"]["sha384"].as_str().unwrap(),
            commitment(&h.commit_nonce, &target),
            "the logged nonce must open the commitment of event {}",
            h.event_seq
        );
        assert!(!ev.payload.to_string().contains("pid-"));
    }
    assert_eq!(
        r.events(hist[1].event_seq, 1).unwrap()[0].event_type,
        "IdentityConfirmed"
    );
    // The log is append-only.
    assert!(
        p.conn()
            .execute("UPDATE identity_decision_log SET student_id = 'pid-c'", [])
            .is_err()
    );
    p.verify_chain(None, &[]).unwrap();
    p.integrity_check().unwrap();
}

#[test]
fn correction_commitments_open_against_their_events() {
    let d = tempfile::tempdir().unwrap();
    let mut p = with_booklet(&d.path().join("Exam.omrproj"));
    let id = p
        .decision(|tx| {
            tx.record_review_action(&ReviewAction {
                booklet_id: "bk-1".into(),
                position: 2,
                old: Response::Unreadable { bubbles: vec![1] },
                new: Response::Single(1),
                reason: Reason::code("AMBIGUOUS_MARK"),
                actor: reviewer(),
                resolves: vec![],
            })
        })
        .unwrap();
    let (old, new, nonce, seq): (String, String, String, i64) = p
        .conn()
        .query_row(
            "SELECT old_response, new_response, commit_nonce, event_seq FROM response_correction \
             WHERE correction_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    let pair = json!({
        "old": serde_json::from_str::<Value>(&old).unwrap(),
        "new": serde_json::from_str::<Value>(&new).unwrap(),
    });
    let ev = &p.reader().events(seq, 1).unwrap()[0];
    assert_eq!(
        ev.payload["change"]["sha384"].as_str().unwrap(),
        commitment(&nonce, &pair)
    );
}

#[test]
fn engines_never_perform_decision_actions() {
    let d = tempfile::tempdir().unwrap();
    let mut p = with_booklet(&d.path().join("Exam.omrproj"));
    let before = p.reader().events(1, 100).unwrap().len();
    let e = p
        .decision(|tx| {
            tx.record_review_action(&ReviewAction {
                booklet_id: "bk-1".into(),
                position: 0,
                old: Response::Blank,
                new: Response::Single(0),
                reason: Reason::code("AMBIGUOUS_MARK"),
                actor: Actor::engine("recog"),
                resolves: vec![],
            })
        })
        .unwrap_err();
    assert!(matches!(e, StoreError::Invalid(_)), "{e}");
    let e = p
        .decision(|tx| {
            tx.put_identity_decision(
                &decision("pid-a", IdentityStatus::Confirmed),
                &Actor::engine("identity"),
                None,
            )
        })
        .unwrap_err();
    assert!(matches!(e, StoreError::Invalid(_)), "{e}");
    let e = p
        .decision(|tx| {
            tx.put_key_version(
                &exam_id(),
                &omr_scoring::KeyVersion::new(vec![omr_scoring::KeyEntry::single(0, 4)]),
                &Actor::engine("recog"),
                None,
            )
        })
        .unwrap_err();
    assert!(matches!(e, StoreError::Invalid(_)), "{e}");
    let e = p
        .decision(|tx| {
            tx.put_policy_version(
                &exam_id(),
                &PolicyVersion::default(),
                &Actor::engine("score"),
            )
        })
        .unwrap_err();
    assert!(matches!(e, StoreError::Invalid(_)), "{e}");
    assert_eq!(p.reader().events(1, 100).unwrap().len(), before);
    // An engine may still propose (auto-accept or ask for confirmation).
    p.decision(|tx| {
        tx.put_identity_decision(
            &decision("pid-a", IdentityStatus::AutoAccepted),
            &Actor::engine("identity"),
            None,
        )
    })
    .unwrap();
}

#[test]
fn only_appendix_a_exceptions_are_stored_or_read_back() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let mut p = project_with_exam(&root);
    // Public fields allow a value that never went through `Exception::new`.
    let forged = Exception {
        code: "XYZ-999".into(),
        ..Exception::must("REG-004", Severity::Warn, Scope::Page)
    };
    let e = p
        .decision(|tx| tx.raise_exception(&forged, &instructor()))
        .unwrap_err();
    assert!(matches!(e, StoreError::Invalid(_)), "{e}");
    let bad_severity = Exception {
        severity: Severity::Info,
        ..Exception::must("REG-004", Severity::Warn, Scope::Page)
    };
    assert!(
        p.decision(|tx| tx.raise_exception(&bad_severity, &instructor()))
            .is_err()
    );
    let ok = Exception::must("REG-004", Severity::Warn, Scope::Page);
    p.decision(|tx| tx.raise_exception(&ok, &instructor()))
        .unwrap();
    assert_eq!(p.reader().exceptions(None).unwrap()[0].exception, ok);
    // A stored row edited outside the store is an integrity failure on read (SYS-010).
    p.conn()
        .execute("UPDATE exception SET code = 'XYZ-999'", [])
        .unwrap();
    let e = p.reader().exceptions(None).unwrap_err();
    assert_eq!(e.exception().unwrap().code, "SYS-010");
}

#[test]
fn a_dedup_address_hit_with_another_sha384_is_never_a_duplicate() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let me = instructor();
    p.decision(|tx| {
        tx.create_batch(
            &BatchRecord {
                batch_id: "b1".into(),
                exam_id: exam_id(),
                header: None,
                campus: None,
                operator: None,
                device_profile: None,
                expected_count: None,
                state: "open".into(),
            },
            &me,
        )
    })
    .unwrap();
    let s1 = p.objects().stage_bytes(b"page-A").unwrap();
    p.decision(|tx| tx.spool_page("b1", "p1", 0, &s1, &me))
        .unwrap();
    // The row's evidence hash no longer matches the bytes behind the address (as after a
    // BLAKE3 collision or an edited row): the rescan must not be dropped as CAP-011.
    p.conn()
        .execute("UPDATE object SET sha384 = ?1", [&"0".repeat(96)])
        .unwrap();
    let s2 = p.objects().stage_bytes(b"page-A").unwrap();
    let e = p
        .decision(|tx| tx.spool_page("b1", "p2", 1, &s2, &me))
        .unwrap_err();
    assert!(matches!(e, StoreError::ObjectCorrupt { .. }), "{e}");
}

#[test]
fn one_policy_version_number_names_one_body() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let me = instructor();
    let v1 = PolicyVersion::default();
    let sha = p
        .decision(|tx| tx.put_policy_version(&exam_id(), &v1, &me))
        .unwrap();
    // Identical body: idempotent.
    assert_eq!(
        p.decision(|tx| tx.put_policy_version(&exam_id(), &v1, &me))
            .unwrap(),
        sha
    );
    // Other content under the same number: refused (a score run names the number).
    let mut other = PolicyVersion::default();
    other.bonus.insert(0);
    assert!(matches!(
        p.decision(|tx| tx.put_policy_version(&exam_id(), &other, &me)),
        Err(StoreError::Conflict(_))
    ));
    other.version = 2;
    p.decision(|tx| tx.put_policy_version(&exam_id(), &other, &me))
        .unwrap();
}

/// A signer whose key store is unavailable.
struct OfflineSigner;

impl CheckpointSigner for OfflineSigner {
    fn algorithm(&self) -> &str {
        omr_store::ALG_ECDSA_P384_SHA384
    }
    fn key_id(&self) -> String {
        "p384:offline".into()
    }
    fn public_key(&self) -> Vec<u8> {
        Vec::new()
    }
    fn sign(&self, _: &[u8]) -> omr_store::Result<Vec<u8>> {
        Err(StoreError::Signing("key store unavailable".into()))
    }
}

#[test]
fn a_failed_automatic_checkpoint_never_reports_a_committed_command_as_unsaved() {
    let d = tempfile::tempdir().unwrap();
    let mut p = new_project(&d.path().join("Exam.omrproj"));
    p.set_signer(Some(Box::new(OfflineSigner)));
    let note = |k: i64| NewEvent::new("NoteAdded", "exam", "e", instructor(), json!({"k": k}));
    // 1,000 events in one command: the commit stands although its checkpoint cannot be signed.
    p.decision(|tx| {
        for k in 0..1000 {
            tx.append_event(&note(k))?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(p.reader().events(1, 2000).unwrap().len(), 1001);
    assert!(
        p.auto_checkpoint_error()
            .is_some_and(|e| e.contains("key store unavailable"))
    );
    assert!(p.reader().checkpoints().unwrap().is_empty());
    // Batch end reports the failure; a working signer then writes the overdue checkpoint.
    assert!(matches!(p.batch_end(), Err(StoreError::Signing(_))));
    p.set_signer(Some(Box::new(test_signer())));
    p.append_event(&note(1000)).unwrap();
    assert_eq!(p.auto_checkpoint_error(), None);
    assert_eq!(p.reader().checkpoints().unwrap()[0].seq, 1002);
}

#[test]
fn a_version_2_project_back_fills_its_identity_decisions_on_migration() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    {
        let mut p = with_booklet(&root);
        p.decision(|tx| {
            tx.put_identity_decision(
                &decision("pid-a", IdentityStatus::AutoAccepted),
                &Actor::engine("identity"),
                None,
            )
        })
        .unwrap();
        p.close().unwrap();
    }
    {
        let c = Connection::open(root.join("project.db")).unwrap();
        c.execute_batch("DROP TABLE identity_decision_log; PRAGMA user_version = 2;")
            .unwrap();
        assert_eq!(
            omr_store::schema::schema_sha384(&c).unwrap(),
            omr_store::schema::canonical_schema_sha384(2).unwrap()
        );
    }
    let p = Project::open(&root, opts()).unwrap();
    let hist = p.reader().identity_history("ir-1", "bk-1").unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].student_id.as_deref(), Some("pid-a"));
    assert_eq!(hist[0].actor_id, "engine:identity");
    let ev = &p.reader().events(hist[0].event_seq, 1).unwrap()[0];
    assert_eq!(ev.event_type, "IdentityDecided");
    let target = json!({"student_id": "pid-a", "status": "auto_accepted"});
    assert_eq!(
        ev.payload["target"]["sha384"].as_str().unwrap(),
        commitment(&hist[0].commit_nonce, &target)
    );
}
