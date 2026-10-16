//! Crash safety of writes (§14.2, §22.2): one transaction per batch, rollback on error and
//! panic, idempotent commands, and a real process abort in the middle of a batch.

mod common;

use common::*;
use omr_model::{LogitMilli, QuestionOutcome, Response};
use omr_store::{
    Actor, BatchRecord, CommandOutcome, NewEvent, PageRecognition, Project, RecognitionRunRecord,
    RunKind, SpoolOutcome, StoreError,
};
use serde_json::json;
use std::path::Path;
use std::process::Command;

const CRASH_ENV: &str = "OMR_STORE_CRASH_CHILD_DIR";

fn batch(p: &mut Project) {
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
                expected_count: Some(6),
                state: "open".into(),
            },
            &me,
        )?;
        tx.create_recognition_run(&RecognitionRunRecord {
            run_id: "rr".into(),
            exam_id: exam_id(),
            kind: RunKind::Provisional,
            parent_run: None,
            engine_manifest: Default::default(),
        })?;
        Ok(())
    })
    .unwrap();
    for k in 0..6 {
        let s = p
            .objects()
            .stage_bytes(format!("page {k}").as_bytes())
            .unwrap();
        let out = p
            .decision(|tx| tx.spool_page("b1", &format!("pg{k}"), k, &s, &Actor::engine("ingest")))
            .unwrap();
        assert!(matches!(out, SpoolOutcome::Spooled { .. }));
    }
}

fn recognition(page: &str) -> PageRecognition {
    PageRecognition {
        page_id: page.into(),
        registration: json!({"matched": 600}),
        questions: (0..20)
            .map(|q| QuestionOutcome {
                question: q,
                response: Response::Single((q % 4) as u8),
                auto_accepted: true,
                logit: LogitMilli(8000),
                exceptions: vec![],
            })
            .collect(),
        exceptions: vec![],
    }
}

fn counts(p: &Project) -> (i64, i64, i64) {
    let q = |sql: &str| p.conn().query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    (
        q("SELECT count(*) FROM question_result"),
        q("SELECT count(*) FROM page WHERE state = 'read'"),
        q("SELECT count(*) FROM events"),
    )
}

#[test]
fn error_inside_a_batch_rolls_back_the_whole_batch() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    batch(&mut p);
    let before = counts(&p);
    let e = p
        .bulk(|tx| {
            tx.record_page_recognition("rr", &recognition("pg0"))?;
            tx.record_page_recognition("rr", &recognition("pg1"))?;
            tx.record_page_recognition("rr", &recognition("no-such-page"))
        })
        .unwrap_err();
    assert!(
        matches!(e, StoreError::NotFound(_) | StoreError::Sqlite(_)),
        "{e}"
    );
    assert_eq!(counts(&p), before);
    p.verify_chain(None, &[]).unwrap();
    // The same batch then commits cleanly.
    p.bulk(|tx| {
        tx.record_page_recognition("rr", &recognition("pg0"))?;
        tx.record_page_recognition("rr", &recognition("pg1"))
    })
    .unwrap();
    let (q, read, ev) = counts(&p);
    assert_eq!((q, read, ev), (40, 2, before.2 + 2));
}

#[test]
fn panic_inside_a_command_rolls_back_and_the_project_stays_usable() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    batch(&mut p);
    let before = counts(&p);
    let e = p
        .bulk(|tx| -> omr_store::Result<()> {
            tx.record_page_recognition("rr", &recognition("pg0"))?;
            panic!("decoder bug");
        })
        .unwrap_err();
    assert!(
        matches!(&e, StoreError::Panicked(m) if m.contains("decoder bug")),
        "{e}"
    );
    assert_eq!(counts(&p), before);
    p.bulk(|tx| tx.record_page_recognition("rr", &recognition("pg0")))
        .unwrap();
    p.verify_chain(None, &[]).unwrap();
    p.integrity_check().unwrap();
}

#[test]
fn a_retried_command_is_a_no_op() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let ev = NewEvent::new("NoteAdded", "exam", "e", instructor(), json!({"k": 1}));
    let first = p.command("cmd-7", |tx| tx.append_event(&ev)).unwrap();
    let seq = match first {
        CommandOutcome::Applied(r) => r.seq,
        other => panic!("{other:?}"),
    };
    let again = p.command("cmd-7", |tx| tx.append_event(&ev)).unwrap();
    assert_eq!(again, CommandOutcome::AlreadyApplied { committed_seq: seq });
    assert_eq!(p.reader().events(1, 100).unwrap().len() as i64, seq);
}

/// The child half of [`process_abort_mid_batch_loses_only_the_open_batch`]: commits one batch,
/// then aborts the process while a second batch is half written. Does nothing unless the
/// parent set the environment variable.
#[test]
#[ignore = "helper run by process_abort_mid_batch_loses_only_the_open_batch"]
fn crash_child() {
    let Some(dir) = std::env::var_os(CRASH_ENV) else {
        return;
    };
    let root = Path::new(&dir).join("Exam.omrproj");
    let mut p = Project::open(&root, opts()).unwrap();
    p.bulk(|tx| {
        tx.record_page_recognition("rr", &recognition("pg0"))?;
        tx.record_page_recognition("rr", &recognition("pg1"))
    })
    .unwrap();
    let _ = p.bulk(|tx| -> omr_store::Result<()> {
        tx.record_page_recognition("rr", &recognition("pg2"))?;
        tx.record_page_recognition("rr", &recognition("pg3"))?;
        // Power loss / kill -9 equivalent: no unwinding, no rollback, no close.
        std::process::abort();
    });
}

#[test]
fn process_abort_mid_batch_loses_only_the_open_batch() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    {
        let mut p = project_with_exam(&root);
        batch(&mut p);
        p.close().unwrap();
    }
    let before = {
        let p = Project::open(&root, opts()).unwrap();
        counts(&p)
    };
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "crash_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_ENV, d.path())
        .status()
        .unwrap();
    assert!(!status.success(), "the child must have aborted");
    // Reopen: WAL recovery, schema, quick_check and the full chain verify.
    let p = Project::open(&root, opts()).unwrap();
    let (q, read, ev) = counts(&p);
    assert_eq!(
        q,
        before.0 + 40,
        "only the committed batch's 2 pages × 20 questions"
    );
    assert_eq!(read, 2);
    assert_eq!(ev, before.2 + 2);
    assert!(p.reader().question_results("rr", "pg2").unwrap().is_empty());
    p.verify_chain(None, &[]).unwrap();
    p.integrity_check().unwrap();
}
