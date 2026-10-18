//! Round trips of every stored entity through a reopened project (§14.3).

mod common;

use common::*;
use omr_model::exceptions::Target;
use omr_model::{
    EngineManifest, Exception, FormCode, LogitMilli, QuestionOutcome, Response, Scope, Severity,
};
use omr_outcomes::{ItemCloMap, MappingMode, MappingSet};
use omr_scoring::{
    Adjustments, BookletL1, Form, FormMap, KeyEntry, KeyVersion, L1Response, MasterItem,
    PolicyVersion, ResponseSnapshot, pts, score,
};
use omr_store::{
    Actor, BatchRecord, BookletRecord, ExportRecord, IdentityDecisionRecord, IdentityRunRecord,
    IdentityStatus, PageRecognition, PageRecord, PageState, PinnedKind, Project, Reason,
    RecognitionRunRecord, ReviewAction, RosterRowIn, RosterSnapshotIn, RunKind, SpoolOutcome,
    StoreError, TemplateOrigin,
};
use serde_json::json;
use std::collections::BTreeMap;

fn form_map() -> FormMap {
    let items: Vec<MasterItem> = (1..=4)
        .map(|i| MasterItem::sba(format!("Q{i}"), 4, pts(1, 1)))
        .collect();
    let a = Form::identity(FormCode(1), &items);
    let b = Form::permuted(
        FormCode(2),
        &[3, 2, 1, 0],
        &[
            vec![1, 0, 2, 3],
            vec![0, 1, 2, 3],
            vec![3, 2, 1, 0],
            vec![0, 1, 2, 3],
        ],
    );
    // Form B first, so the stored order (not the code order) must be kept.
    FormMap {
        items,
        forms: vec![b, a],
        per_form_normalization: false,
    }
}

fn key() -> KeyVersion {
    KeyVersion::new(vec![
        KeyEntry::single(0, 4),
        KeyEntry::single(1, 4),
        KeyEntry::single(2, 4),
        KeyEntry::single(3, 4),
    ])
}

fn roster() -> RosterSnapshotIn {
    RosterSnapshotIn {
        parent: None,
        source: "sis-export".into(),
        fetched_at: "2026-10-01T08:00:00Z".into(),
        id_scheme_version: "test-v1".into(),
        rows: vec![
            RosterRowIn {
                student_id: "pid-b".into(),
                section: "101".into(),
                sis_status: "enrolled".into(),
                fields: json!({"name_en": "Student Two", "name_ar": "طالب اثنان"}),
                attributes: BTreeMap::from([("track".into(), "ordinary".into())]),
            },
            RosterRowIn {
                student_id: "pid-a".into(),
                section: "101".into(),
                sis_status: "enrolled".into(),
                fields: json!({"name_en": "Student One", "name_ar": "طالب واحد"}),
                attributes: BTreeMap::new(),
            },
        ],
    }
}

fn outcome(q: u16, response: Response, auto: bool, codes: &[&str]) -> QuestionOutcome {
    QuestionOutcome {
        question: q,
        response,
        auto_accepted: auto,
        logit: LogitMilli(if auto { 9000 } else { 1200 }),
        exceptions: codes
            .iter()
            .map(|c| {
                Exception::must(c, Severity::Error, Scope::Item).with_target(Target {
                    question: Some(q),
                    ..Target::default()
                })
            })
            .collect(),
    }
}

#[test]
fn every_entity_round_trips_through_a_reopened_project() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let me = instructor();
    let recog_run = "rr-1";
    let (tmpl_sha, fm_sha, pol_sha, roster_sha, map_sha, pin_sha);
    let fm = form_map();
    let policy = PolicyVersion::default();
    let mapping = MappingSet {
        version: 1,
        mode: MappingMode::Full,
        rows: vec![
            ItemCloMap::approved("mt", 0, "CLO1", pts(1, 1)),
            ItemCloMap::approved("mt", 1, "CLO2", pts(1, 2)),
        ],
    };
    let page_bytes = b"synthetic page image bytes".to_vec();
    let registration = json!({"matched": 687, "rms_px_milli": 412, "scale_milli": 907});
    let l1 = ResponseSnapshot {
        booklets: vec![
            BookletL1::present(
                1,
                Some("pid-a"),
                FormCode(1),
                vec![
                    L1Response::Single(0),
                    L1Response::Single(1),
                    L1Response::Single(3),
                    L1Response::Blank,
                ],
            ),
            BookletL1::absent(2, Some("pid-b"), FormCode(2), true),
        ],
    };
    let run = score(&l1, &fm, &key(), &policy, &Adjustments::default()).unwrap();
    {
        let mut p = project_with_exam(&root);
        let staged = p.objects().stage_bytes(&page_bytes).unwrap();
        assert!(!staged.already_present);
        let ex = exam_id();
        (tmpl_sha, fm_sha, pol_sha, roster_sha, map_sha, pin_sha) = p
            .decision(|tx| {
                let t = tx.put_template_revision(
                    "cs101-sheet",
                    TemplateOrigin::Native,
                    "CS 101 sheet",
                    1,
                    &json!({"questions": 4, "options": 4}),
                    &me,
                )?;
                // Idempotent for identical content; conflicting content is refused.
                assert_eq!(
                    tx.put_template_revision(
                        "cs101-sheet",
                        TemplateOrigin::Native,
                        "CS 101 sheet",
                        1,
                        &json!({"questions": 4, "options": 4}),
                        &me
                    )?,
                    t
                );
                assert!(matches!(
                    tx.put_template_revision(
                        "cs101-sheet",
                        TemplateOrigin::Native,
                        "x",
                        1,
                        &json!({"questions": 5}),
                        &me
                    ),
                    Err(StoreError::Conflict(_))
                ));
                let f = tx.put_form_map(&ex, &fm, Some(&t), "en", &me)?;
                tx.put_key_version(&ex, &key(), &me, None)?;
                let k2 = key().revise([(2, KeyEntry::any_of(&[2, 3], 4))], "KEY_ERROR");
                tx.put_key_version(&ex, &k2, &me, Some(Reason::code("KEY_ERROR")))?;
                let pol = tx.put_policy_version(&ex, &policy, &me)?;
                let r = tx.put_roster_snapshot(&roster(), &me)?;
                let pin = tx.pin(
                    PinnedKind::GradeScale,
                    &omr_scoring::GradeScale::saudi(),
                    &me,
                )?;
                let m = tx.put_mapping_set(&mapping, &me)?;
                tx.create_batch(
                    &BatchRecord {
                        batch_id: "b1".into(),
                        exam_id: ex,
                        header: Some("H-0001".into()),
                        campus: Some("main".into()),
                        operator: Some("principal:scan-op".into()),
                        device_profile: Some("adf-1".into()),
                        expected_count: Some(2),
                        state: "open".into(),
                    },
                    &me,
                )?;
                let out = tx.spool_page("b1", "pg-1", 0, &staged, &Actor::engine("ingest"))?;
                assert_eq!(
                    out,
                    SpoolOutcome::Spooled {
                        page_id: "pg-1".into()
                    }
                );
                tx.create_booklet(
                    &BookletRecord {
                        booklet_id: "bk-1".into(),
                        exam_id: ex,
                        serial: 1,
                        form_code: Some(1),
                        bound_student: None,
                        state: "collated".into(),
                    },
                    &me,
                )?;
                tx.attach_page("pg-1", "bk-1", 1, &me)?;
                tx.create_recognition_run(&RecognitionRunRecord {
                    run_id: recog_run.into(),
                    exam_id: ex,
                    kind: RunKind::Provisional,
                    parent_run: None,
                    engine_manifest: EngineManifest {
                        app_version: "0.1.0".into(),
                        template_sha384: t.clone(),
                        config_sha384: "c".repeat(96),
                        ..EngineManifest::default()
                    },
                })?;
                Ok((t, f, pol, r, m, pin))
            })
            .unwrap();
        p.bulk(|tx| {
            tx.record_page_recognition(
                recog_run,
                &PageRecognition {
                    page_id: "pg-1".into(),
                    registration: registration.clone(),
                    questions: vec![
                        outcome(0, Response::Single(0), true, &[]),
                        outcome(1, Response::Multi(vec![1, 2]), false, &["MRK-002"]),
                        outcome(
                            2,
                            Response::Unreadable { bubbles: vec![3] },
                            false,
                            &["MRK-007", "MRK-003"],
                        ),
                        outcome(3, Response::Blank, true, &[]),
                    ],
                    exceptions: vec![Exception::must("REG-004", Severity::Warn, Scope::Page)],
                },
            )
        })
        .unwrap();
        let open = p.reader().exceptions(Some("open")).unwrap();
        let mrk = open
            .iter()
            .find(|e| e.exception.code == "MRK-002")
            .unwrap()
            .exception_id;
        p.decision(|tx| {
            tx.create_identity_run(&IdentityRunRecord {
                run_id: "ir-1".into(),
                exam_id: ex,
                roster_snapshot: Some(roster_sha.clone()),
                models: json!({"grid": "v1"}),
                thresholds: json!({"auto_accept_q": 7066}),
            })?;
            tx.put_identity_decision(
                &IdentityDecisionRecord {
                    run_id: "ir-1".into(),
                    booklet_id: "bk-1".into(),
                    student_id: Some("pid-a".into()),
                    status: IdentityStatus::NeedsConfirmation,
                    logit_q: Some(5120),
                    margin_q: Some(4000),
                    evidence: json!({"channels": ["grid"], "candidates": 2}),
                },
                &Actor::engine("identity"),
                None,
            )?;
            tx.record_review_action(&ReviewAction {
                booklet_id: "bk-1".into(),
                position: 1,
                old: Response::Multi(vec![1, 2]),
                new: Response::Single(1),
                reason: Reason::code("ERASURE_RESIDUE"),
                actor: Actor::user("principal:reviewer", "Reviewer"),
                resolves: vec![mrk],
            })?;
            tx.put_score_run(&ex, &run, &me)?;
            assert!(!tx.put_score_run(&ex, &run, &me)?); // idempotent by Run ID
            tx.record_export(
                &ExportRecord {
                    export_id: "x-1".into(),
                    purpose: "grades".into(),
                    destination: "folder:exports".into(),
                    recipient: None,
                    pii: true,
                    label: "restricted".into(),
                    manifest_sha384: "a".repeat(96),
                },
                &me,
            )?;
            Ok(())
        })
        .unwrap();
        p.close().unwrap();
    }

    let p = Project::open(&root, opts()).unwrap();
    let r = p.reader();
    let ex = exam_id();
    assert_eq!(r.exam(&ex).unwrap(), exam_record());
    assert_eq!(
        r.template_revision(&tmpl_sha).unwrap(),
        json!({"questions": 4, "options": 4})
    );
    assert_eq!(r.form_map(&ex).unwrap(), fm);
    assert_eq!(omr_store::jcs::jcs_sha384_hex(&fm).unwrap(), fm_sha);
    assert_eq!(r.key_versions(&ex).unwrap(), vec![1, 2]);
    assert_eq!(r.key_version(&ex, 1).unwrap(), key());
    assert_eq!(r.key_version(&ex, 2).unwrap().reason, "KEY_ERROR");
    assert_eq!(r.policy_version(&pol_sha).unwrap(), policy);
    let rows = r.roster_rows(&roster_sha).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].student_id, "pid-a");
    assert_eq!(rows[1].fields["name_ar"], "طالب اثنان");
    assert_eq!(
        r.pinned(PinnedKind::GradeScale, &pin_sha).unwrap(),
        serde_json::to_value(omr_scoring::GradeScale::saudi()).unwrap()
    );
    assert_eq!(r.mapping_set(&map_sha).unwrap(), mapping);
    let b = r.batch("b1").unwrap();
    assert_eq!((b.expected_count, b.state.as_str()), (Some(2), "open"));
    let page: PageRecord = r.page("pg-1").unwrap();
    assert_eq!(page.state, PageState::Read);
    assert_eq!(page.booklet_id.as_deref(), Some("bk-1"));
    assert_eq!(page.registration, Some(registration));
    assert_eq!(
        p.objects()
            .read_verified(&page.object, &page.source_sha384)
            .unwrap(),
        page_bytes
    );
    let qs = r.question_results(recog_run, "pg-1").unwrap();
    assert_eq!(qs.len(), 4);
    assert_eq!(qs[1].response, Response::Multi(vec![1, 2]));
    assert!(!qs[1].auto_accepted);
    assert_eq!(qs[2].codes, vec!["MRK-003", "MRK-007"]);
    assert_eq!(qs[2].response, Response::Unreadable { bubbles: vec![3] });
    assert_eq!(qs[0].logit, LogitMilli(9000));
    let all = r.exceptions(None).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(r.exceptions(Some("open")).unwrap().len(), 3);
    let reg = all.iter().find(|e| e.exception.code == "REG-004").unwrap();
    assert_eq!(reg.exception.scope, Scope::Page);
    let ids = r.identity_decisions("ir-1").unwrap();
    assert_eq!(ids[0].student_id.as_deref(), Some("pid-a"));
    assert_eq!(ids[0].status, IdentityStatus::NeedsConfirmation);
    let corr = r.corrections("bk-1").unwrap();
    assert_eq!(corr.len(), 1);
    assert_eq!(corr[0].new, Response::Single(1));
    assert_eq!(corr[0].reason_code, "ERASURE_RESIDUE");
    assert_eq!(r.score_run(&run.run_id).unwrap(), run);
    assert_eq!(r.exports().unwrap()[0].manifest_sha384, "a".repeat(96));
    // Events carry no plaintext PII or student IDs.
    let events = r.events(1, 1000).unwrap();
    let all_payloads =
        serde_json::to_string(&events.iter().map(|e| &e.payload).collect::<Vec<_>>()).unwrap();
    for needle in ["pid-a", "pid-b", "Student One", "طالب"] {
        assert!(
            !all_payloads.contains(needle),
            "{needle} leaked into an event payload"
        );
    }
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    for t in [
        "ProjectCreated",
        "ExamCreated",
        "TemplateRevisionFrozen",
        "FormMapDefined",
        "KeyVersionPublished",
        "PolicyVersionPublished",
        "RosterSnapshotImported",
        "Pinned",
        "MappingSetPublished",
        "BatchOpened",
        "PageSpooled",
        "BookletCollated",
        "PageAttached",
        "RecognitionRunStarted",
        "PageRecognized",
        "IdentityRunStarted",
        "IdentityDecided",
        "ResponseCorrected",
        "ExceptionResolved",
        "ScoreRunComputed",
        "ExportCreated",
    ] {
        assert!(types.contains(&t), "missing event {t}");
    }
    let key_ev = events
        .iter()
        .find(|e| e.event_type == "KeyVersionPublished" && e.payload["n"] == 2)
        .unwrap();
    assert_eq!(key_ev.reason_code.as_deref(), Some("KEY_ERROR"));
    p.verify_chain(None, &[]).unwrap();
    p.integrity_check().unwrap();
    assert_eq!(p.verify_objects().unwrap(), 1);
}

#[test]
fn append_only_and_conflicting_writes_are_refused() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let ex = exam_id();
    let me = instructor();
    p.decision(|tx| tx.put_key_version(&ex, &key(), &me, None))
        .unwrap();
    // Republishing version 1 (with other content) is refused, and nothing else is written.
    let before = p.reader().events(1, 100).unwrap().len();
    let mut other = key();
    other.entries[0] = Some(KeyEntry::single(3, 4));
    let e = p
        .decision(|tx| tx.put_key_version(&ex, &other, &me, None))
        .unwrap_err();
    assert!(matches!(e, StoreError::Conflict(_)));
    assert_eq!(p.reader().events(1, 100).unwrap().len(), before);
    // SQL updates of a key version are blocked by the append-only trigger.
    assert!(
        p.conn()
            .execute("UPDATE key_version SET reason = 'x'", [])
            .is_err()
    );
    // A form map is immutable once stored.
    p.decision(|tx| tx.put_form_map(&ex, &form_map(), None, "en", &me))
        .unwrap();
    let mut fm2 = form_map();
    fm2.per_form_normalization = true;
    assert!(matches!(
        p.decision(|tx| tx.put_form_map(&ex, &fm2, None, "en", &me)),
        Err(StoreError::Conflict(_))
    ));
    // Unknown reason codes and notes-required codes are refused.
    assert!(
        p.decision(|tx| tx.put_key_version(
            &ex,
            &key().revise([], "x"),
            &me,
            Some(Reason::code("BECAUSE"))
        ))
        .is_err()
    );
}

#[test]
fn byte_identical_rescan_is_cap_011_and_adds_no_page() {
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
    let s2 = p.objects().stage_bytes(b"page-A").unwrap();
    assert!(!s1.already_present && s2.already_present);
    assert_eq!(s1.blake3, s2.blake3);
    assert_eq!(s1.sha384, s2.sha384);
    let a = p
        .decision(|tx| tx.spool_page("b1", "p1", 0, &s1, &me))
        .unwrap();
    assert!(matches!(a, SpoolOutcome::Spooled { .. }));
    let b = p
        .decision(|tx| tx.spool_page("b1", "p2", 1, &s2, &me))
        .unwrap();
    match b {
        SpoolOutcome::Duplicate {
            existing_pages,
            exception,
        } => {
            assert_eq!(existing_pages, vec!["p1".to_string()]);
            assert_eq!(
                (exception.code.as_str(), exception.severity),
                ("CAP-011", Severity::Info)
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(p.reader().pages("b1").unwrap().len(), 1);
    p.decision(|tx| tx.close_batch("b1", &me)).unwrap();
    assert_eq!(p.reader().batch("b1").unwrap().state, "closed");
}

#[test]
fn object_store_detects_corruption_and_sweeps_orphans() {
    let d = tempfile::tempdir().unwrap();
    let mut p = project_with_exam(&d.path().join("Exam.omrproj"));
    let kept = p.objects().stage_bytes(b"kept").unwrap();
    p.decision(|tx| tx.insert_object(&kept, omr_store::ObjectKind::Original))
        .unwrap();
    // An orphan: staged (durable) but its row never inserted (a crash between steps 4 and 5).
    let orphan = p.objects().stage_bytes(b"orphan").unwrap();
    assert_eq!(p.verify_objects().unwrap(), 1);
    let man = p.object_manifest().unwrap();
    assert_eq!(man.entries.len(), 1);
    assert_eq!(man.entries[0].sha384, kept.sha384);
    assert_eq!(man.sha384().unwrap().len(), 96);
    // Not old enough yet.
    assert!(
        p.sweep_orphans(std::time::Duration::from_secs(3600))
            .unwrap()
            .orphans_removed
            .is_empty()
    );
    let rep = p.sweep_orphans(std::time::Duration::ZERO).unwrap();
    assert_eq!(rep.orphans_removed, vec![orphan.blake3.clone()]);
    assert_eq!(p.objects().list().unwrap(), vec![kept.blake3.clone()]);
    // Flip one byte of the kept object: SHA-384 verification fails.
    let path = p.objects().path_of(&kept.blake3).unwrap();
    std::fs::write(&path, b"kepT").unwrap();
    assert!(matches!(
        p.verify_objects(),
        Err(StoreError::ObjectCorrupt { .. })
    ));
    assert!(
        p.objects()
            .read_verified(&kept.blake3, &kept.sha384)
            .is_err()
    );
}

#[test]
fn objects_land_at_their_blake3_address() {
    let d = tempfile::tempdir().unwrap();
    let p = new_project(&d.path().join("Exam.omrproj"));
    let s = p.objects().stage_bytes(b"abc").unwrap();
    // BLAKE3("abc") and SHA-384("abc") reference values.
    assert_eq!(
        s.blake3,
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    );
    assert!(
        s.sha384
            .starts_with("cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163")
    );
    let path = p.objects().path_of(&s.blake3).unwrap();
    assert!(path.ends_with(
        "objects/64/37/6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    ));
    assert!(
        std::fs::read_dir(&p.paths().spool)
            .unwrap()
            .next()
            .is_none()
    );
    assert!(p.objects().path_of("../../etc/passwd").is_err());
}
