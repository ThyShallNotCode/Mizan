//! Resolving and reopening exceptions without a response correction (identity, form and
//! page-level review decisions; undo as a compensating event, §4.7, §13.1, §14.4).

mod common;

use common::*;
use omr_model::{Exception, Scope, Severity};
use omr_store::{Actor, Reason, StoreError};

#[test]
fn exceptions_resolve_and_reopen_through_events_only() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let me = instructor();
    let id = p
        .decision(|tx| {
            tx.raise_exception(
                &Exception::must("IDN-003", Severity::Error, Scope::Booklet),
                &Actor::engine("scoring"),
            )
        })
        .unwrap();
    let open = |p: &omr_store::Project| {
        p.reader()
            .exceptions(Some("open"))
            .unwrap()
            .iter()
            .filter(|e| e.exception_id == id)
            .count()
    };
    assert_eq!(open(&p), 1);
    // Engines never resolve; humans do, with a reason code.
    assert!(matches!(
        p.decision(|tx| tx.resolve_exceptions(
            &[id],
            &Actor::engine("x"),
            Reason::code("CLERICAL_CORRECTION"),
            "form"
        )),
        Err(StoreError::Invalid(_))
    ));
    p.decision(|tx| tx.resolve_exceptions(&[id], &me, Reason::code("CLERICAL_CORRECTION"), "form"))
        .unwrap();
    assert_eq!(open(&p), 0);
    // A second resolve finds nothing open.
    assert!(
        p.decision(|tx| tx.resolve_exceptions(
            &[id],
            &me,
            Reason::code("CLERICAL_CORRECTION"),
            "form"
        ))
        .is_err()
    );
    p.decision(|tx| tx.reopen_exceptions(&[id], &me, Reason::code("CLERICAL_CORRECTION")))
        .unwrap();
    assert_eq!(open(&p), 1);
    let types: Vec<String> = p
        .reader()
        .events(1, 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.event_type)
        .collect();
    assert!(types.iter().any(|t| t == "ExceptionResolved"));
    assert!(types.iter().any(|t| t == "ExceptionReopened"));
    p.verify_chain(None, &[]).unwrap();
}
