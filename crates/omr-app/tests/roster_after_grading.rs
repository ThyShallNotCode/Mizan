//! G-3 D8: a roster added to an exam graded without one (synthetic IAU sheets and rosters
//! only). The exam is identified again over its stored originals, exactly as if it had been
//! graded with the roster from the start; review decisions carry over; a roster that is
//! refused, or a run that fails, changes nothing; the roster is audited (its snapshot, the
//! identity run that used it, the release it superseded) and the exam reopens clean; once
//! released again, the Blackboard upload is written.

mod common;

use common::*;
use omr_app::review::ReviewReason;
use omr_app::{
    Action, AppError, Decision, ExamDefinition, ExamProject, FormKey, ItemKind, ItemState,
    KeySource, RosterStudent,
};
use omr_forms::legacy::LegacyField;
use std::path::{Path, PathBuf};

const N: u16 = 20;
const SHEETS: u16 = 8;

fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

fn answer(k: u16, q: u16) -> Option<u8> {
    let v = k % 2;
    if (q + k) % 11 == 7 {
        None
    } else if q.is_multiple_of(k + 2) {
        Some((key(v, q) + 1) % 5)
    } else {
        Some(key(v, q))
    }
}

/// IAU student numbers far apart: no sheet's number is a writer's slip from another's.
fn student(k: u16) -> String {
    format!(
        "221{:07}",
        (u32::from(k) * 1_234_567 + 1_111_111) % 10_000_000
    )
}

/// Sheets `range` into `dir`; sheets 1 and 6 carry a faint second mark (review items).
fn sheets(dir: &Path, range: std::ops::Range<u16>) -> PathBuf {
    let e = iau();
    for k in range {
        let answers: Vec<Option<u8>> = (0..N).map(|q| answer(k, q)).collect();
        let mut m = Marks::new(&answers, &student(k), Some((k % 2) as u8));
        if k == 1 || k == 6 {
            let o = (answer(k, 3).unwrap() + 2) % 5;
            m.light.push(bubble(&e, LegacyField::Answers, 3, o));
        }
        write(
            dir,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, &m, 900 + u64::from(k))),
        );
    }
    dir.to_path_buf()
}

/// The exam, without a roster.
fn definition() -> ExamDefinition {
    legacy_def(
        "Midterm",
        N,
        &["A", "B"],
        KeySource::Provided {
            forms: (0..2u16)
                .map(|v| FormKey {
                    form: ["A", "B"][usize::from(v)].into(),
                    answers: (0..N)
                        .map(|q| char::from(b'A' + key(v, q)).to_string())
                        .collect(),
                })
                .collect(),
        },
    )
}

/// Roster rows of the students `ks`.
fn rows(ks: &[u16]) -> Vec<RosterStudent> {
    ks.iter()
        .map(|&k| RosterStudent {
            id: student(k),
            name: format!("Student {k}"),
            name_ar: String::new(),
            section: "101".into(),
            numeric_cell: false,
        })
        .collect()
}

fn sheet_no(p: &ExamProject, bi: usize) -> u16 {
    source_of(p, bi)[6..8].parse().unwrap()
}

fn light_item(p: &ExamProject, k: u16) -> u64 {
    p.queue(None)
        .iter()
        .find(|i| {
            i.kind == ItemKind::Mark
                && i.question == Some(3)
                && sheet_no(p, i.booklet.unwrap()) == k
        })
        .map(|i| i.id)
        .expect("light mark reviewed")
}

/// The reviewer's decisions: sheet 1's light mark decided, sheet 6's deferred.
fn review(p: &mut ExamProject) {
    let id = light_item(p, 1);
    let truth = Decision::Option(answer(1, 3).unwrap());
    let reason = p
        .overrides_engine(id, &truth)
        .then(|| ReviewReason::code("ERASURE_RESIDUE"));
    p.apply_with_reason(
        id,
        Action::Resolve {
            decision: truth,
            accepted_suggestion: false,
            note: String::new(),
        },
        reason,
    )
    .unwrap();
    p.apply(light_item(p, 6), Action::Defer).unwrap();
}

/// Every open item decided as the sheet was marked.
fn resolve_all(p: &mut ExamProject) {
    let open: Vec<(u64, ItemKind, Option<usize>, Option<u16>)> = p
        .queue(None)
        .iter()
        .map(|i| (i.id, i.kind, i.booklet, i.question))
        .collect();
    for (id, kind, b, q) in open {
        let k = || sheet_no(p, b.unwrap());
        let d = match kind {
            ItemKind::Mark => answer(k(), q.unwrap()).map_or(Decision::Blank, Decision::Option),
            ItemKind::Identity => Decision::Student(student(k())),
            ItemKind::Form => Decision::Form((k() % 2) as u8),
            ItemKind::Notice => Decision::Acknowledge,
        };
        let reason = p
            .overrides_engine(id, &d)
            .then(|| ReviewReason::code("ERASURE_RESIDUE"));
        p.apply_with_reason(
            id,
            Action::Resolve {
                decision: d,
                accepted_suggestion: false,
                note: String::new(),
            },
            reason,
        )
        .unwrap();
    }
}

fn events(p: &ExamProject, ty: &str) -> Vec<omr_store::EventRecord> {
    p.store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == ty)
        .collect()
}

/// The roster snapshot the latest identity run used.
fn identity_roster(p: &ExamProject) -> Option<String> {
    events(p, "IdentityRunStarted")
        .last()
        .and_then(|e| e.payload["roster_snapshot"].as_str().map(str::to_string))
}

#[test]
fn a_roster_after_grading_matches_the_students_and_keeps_the_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"), 0..SHEETS);
    // Sheet 7's student is not on the roster; two students on it handed in nothing.
    let roster = rows(&[0, 1, 2, 3, 4, 5, 6, 20, 21]);
    // Reference: graded with the roster from the start, then reviewed.
    let mut with = definition();
    with.import_roster(roster.clone()).unwrap();
    let ref_out = dir.path().join("ref");
    let mut r = grade(&with, std::slice::from_ref(&scans), &ref_out, 2);
    assert_eq!(
        identity_roster(&r),
        Some(events(&r, "RosterSnapshotImported")[0].entity_id.clone()),
        "an exam graded with its roster names it too"
    );
    review(&mut r);
    r.export(&ref_out).unwrap();
    // Graded without a roster and reviewed; the roster comes later (another process).
    let out = dir.path().join("late");
    let mut p = grade(&definition(), &[scans], &out, 2);
    assert!(
        p.absent_students().is_empty(),
        "no roster: nobody is absent"
    );
    review(&mut p);
    p.close().unwrap();
    let mut p = ExamProject::open(&out.join("project.omrproj"), "principal:clerk").unwrap();
    let report = p
        .set_roster(roster.clone(), &options(&out, 1), &|_| {})
        .unwrap();
    assert_eq!(
        (
            report.students,
            report.matched,
            report.not_on_roster,
            report.absent,
            report.carried_over,
            report.conflicts
        ),
        (9, 7, 1, 2, 1, 0)
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    p.export(&out).unwrap();
    // Exactly as graded with the roster from the start.
    assert_eq!(p.session.definition.roster, r.session.definition.roster);
    assert_eq!(p.session.booklets, r.session.booklets);
    let states = |p: &ExamProject| -> Vec<_> {
        p.session
            .review
            .items
            .iter()
            .map(|i| (i.id, i.kind, i.code.clone(), i.state, i.resolution.clone()))
            .collect()
    };
    assert_eq!(states(&p), states(&r));
    for f in ["results.csv", "booklets.json", "item_analysis.csv"] {
        assert!(
            std::fs::read(out.join(f)).unwrap() == std::fs::read(ref_out.join(f)).unwrap(),
            "{f} differs"
        );
    }
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let k = sheet_no(&p, bi);
        let want = (k < 7).then(|| student(k));
        assert_eq!(b.identity.student, want, "sheet {k}");
    }
    let absent: Vec<&str> = p.absent_students().iter().map(|s| s.id.as_str()).collect();
    assert_eq!(absent, [student(20), student(21)]);
    // The decisions carried over: sheet 1's answer stands, sheet 6's item is still deferred.
    let item = |k: u16| {
        p.session
            .review
            .items
            .iter()
            .find(|i| {
                i.kind == ItemKind::Mark
                    && i.question == Some(3)
                    && sheet_no(&p, i.booklet.unwrap()) == k
            })
            .unwrap()
    };
    assert_eq!(item(1).state, ItemState::Resolved);
    assert_eq!(item(6).state, ItemState::Deferred);
    // Audited: the roster snapshot, the identity run that used it, no student data in events.
    let imported = events(&p, "RosterSnapshotImported");
    assert_eq!(imported.len(), 1);
    let sha = imported[0].entity_id.clone();
    let replaced = events(&p, "RosterReplaced");
    assert_eq!(replaced.len(), 1);
    let ev = &replaced[0].payload;
    assert_eq!(ev["rows"], 9);
    assert_eq!(ev["sha384"], sha.as_str());
    assert!(ev["previous"].is_null());
    assert_eq!(ev["identity_mode"], "roster");
    assert_eq!(identity_roster(&p), Some(sha));
    let all = serde_json::to_string(
        &p.store()
            .unwrap()
            .events()
            .unwrap()
            .iter()
            .map(|e| &e.payload)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!all.contains("Student "), "no names in the event log");
    p.store().unwrap().verify().unwrap();
}

#[test]
fn a_refused_roster_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"), 0..SHEETS);
    let out = dir.path().join("out");
    let mut p = grade(&definition(), &[scans], &out, 2);
    review(&mut p);
    let before = (
        p.session.booklets.clone(),
        p.session.review.items.clone(),
        p.store().unwrap().event_count(),
    );
    let same = |p: &ExamProject| {
        assert!(p.session.definition.roster.is_none());
        assert_eq!(p.session.booklets, before.0);
        assert_eq!(p.session.review.items, before.1);
        assert_eq!(p.store().unwrap().event_count(), before.2);
    };
    // A student listed twice: the roster is refused with the rows to fix.
    let mut twice = rows(&[0, 1, 2]);
    twice.push(twice[1].clone());
    let e = p.set_roster(twice, &options(&out, 1), &|_| {}).unwrap_err();
    assert!(e.to_string().contains("roster"), "{e}");
    same(&p);
    // A run that fails (here cancelled) keeps the exam as it was: no roster, no snapshot.
    let opts = options(&out, 1);
    opts.cancel.cancel();
    let e = p
        .set_roster(rows(&[0, 1, 2, 3, 4, 5, 6, 7]), &opts, &|_| {})
        .unwrap_err();
    assert!(matches!(e, AppError::Cancelled), "{e}");
    same(&p);
    // A run that cannot read the stored originals: refused before it starts, and the roster
    // it was given is not recorded by the next run (here a file added again, CAP-011).
    let objects = out.join("project.omrproj").join("objects");
    let away = out.join("objects-away");
    std::fs::rename(&objects, &away).unwrap();
    assert!(
        p.set_roster(rows(&[0, 1, 2]), &options(&out, 1), &|_| {})
            .is_err()
    );
    same(&p);
    std::fs::rename(&away, &objects).unwrap();
    let again = dir.path().join("again");
    std::fs::create_dir_all(&again).unwrap();
    std::fs::copy(
        dir.path().join("scans").join("sheet-02.png"),
        again.join("sheet-02-copy.png"),
    )
    .unwrap();
    let a = p.add_scans(&[again], &options(&out, 1), &|_| {}).unwrap();
    assert_eq!((a.duplicates, a.booklets_after), (1, usize::from(SHEETS)));
    assert!(p.session.definition.roster.is_none());
    assert!(events(&p, "RosterSnapshotImported").is_empty());
    assert!(events(&p, "RosterReplaced").is_empty());
    assert_eq!(identity_roster(&p), None);
    // The exam stays usable: the roster is set next time, and only that roster is recorded.
    let r = p
        .set_roster(rows(&[0, 1, 2, 3, 4, 5, 6, 7]), &options(&out, 1), &|_| {})
        .unwrap();
    assert_eq!((r.matched, r.not_on_roster, r.absent), (8, 0, 0));
    assert_eq!(events(&p, "RosterSnapshotImported").len(), 1);
    assert_eq!(
        identity_roster(&p),
        Some(events(&p, "RosterSnapshotImported")[0].entity_id.clone())
    );
    p.close().unwrap();
    ExamProject::open(&out.join("project.omrproj"), "principal:test")
        .unwrap()
        .check_audited()
        .unwrap();
}

#[test]
fn the_blackboard_upload_appears_after_the_roster_with_a_provided_key_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"), 0..SHEETS);
    let out = dir.path().join("out");
    let mut p = grade(&definition(), &[scans], &out, 2);
    resolve_all(&mut p);
    p.release(None).unwrap();
    p.export(&out).unwrap();
    let upload = out.join("blackboard_grade_center.csv");
    assert!(!upload.exists(), "a rosterless exam is never uploaded");
    // The roster supersedes the release (identities changed): no upload until released again.
    let everyone: Vec<u16> = (0..SHEETS).chain([20]).collect();
    let r = p
        .set_roster(rows(&everyone), &options(&out, 2), &|_| {})
        .unwrap();
    assert_eq!((r.matched, r.absent), (usize::from(SHEETS), 1));
    assert!(!p.session.released());
    let superseded = events(&p, "ReleaseSuperseded");
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].payload["change"], "roster replaced");
    p.export(&out).unwrap();
    assert!(!upload.exists());
    p.release(None).unwrap();
    p.export(&out).unwrap();
    let text = String::from_utf8(std::fs::read(&upload).unwrap()[3..].to_vec()).unwrap();
    let mut users: Vec<String> = text
        .split("\r\n")
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| l.split(',').next().unwrap().trim_matches('"').to_string())
        .collect();
    users.sort();
    let mut want: Vec<String> = (0..SHEETS).map(student).collect();
    want.sort();
    assert_eq!(
        users, want,
        "every student who handed in a sheet, by roster ID"
    );
}

#[test]
fn an_exam_given_a_roster_reopens_clean_and_keeps_it_for_later_runs() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"), 0..SHEETS - 1);
    let out = dir.path().join("out");
    let pkg = out.join("project.omrproj");
    let p = grade(&definition(), &[scans], &out, 2);
    p.close().unwrap();
    let mut p = ExamProject::open(&pkg, "principal:test").unwrap();
    let first: Vec<u16> = (0..SHEETS).collect();
    p.set_roster(rows(&first), &options(&out, 2), &|_| {})
        .unwrap();
    let sha1 = identity_roster(&p).expect("the identity run names its roster");
    p.close().unwrap();
    // Reopened (the snapshot is checked against the audit log), also read-only.
    ExamProject::open_read_only(&pkg).unwrap();
    let mut p = ExamProject::open(&pkg, "principal:test").unwrap();
    p.check_audited().unwrap();
    assert_eq!(p.absent_students().len(), 1);
    // The exam row keeps the identity mode it was created with; the roster in force is the
    // one the latest identity run names.
    let st = p.store().unwrap();
    let exam = st.reader().exam(&st.exam()).unwrap();
    assert_eq!(exam.identity_mode, omr_store::IdentityMode::CandidateNumber);
    // A later batch is identified against the roster in force, and names it.
    let late = sheets(&dir.path().join("late"), SHEETS - 1..SHEETS);
    let a = p.add_scans(&[late], &options(&out, 2), &|_| {}).unwrap();
    assert_eq!(a.booklets_after, usize::from(SHEETS));
    assert_eq!(identity_roster(&p), Some(sha1.clone()));
    assert!(p.absent_students().is_empty());
    // A roster replaced: the new snapshot's parent is the one it replaces.
    let second: Vec<u16> = (0..SHEETS).chain([30]).collect();
    let r = p
        .set_roster(rows(&second), &options(&out, 2), &|_| {})
        .unwrap();
    assert_eq!(
        (r.students, r.matched, r.absent),
        (9, usize::from(SHEETS), 1)
    );
    let replaced = events(&p, "RosterReplaced");
    assert_eq!(replaced.len(), 2);
    assert_eq!(replaced[1].payload["previous"], sha1.as_str());
    let imported = events(&p, "RosterSnapshotImported");
    assert_eq!(imported[1].payload["parent"], sha1.as_str());
    assert_eq!(identity_roster(&p), Some(imported[1].entity_id.clone()));
    p.close().unwrap();
    let p = ExamProject::open(&pkg, "principal:test").unwrap();
    p.check_audited().unwrap();
    p.store().unwrap().verify().unwrap();
    assert_eq!(p.session.definition.roster.as_ref().map(Vec::len), Some(9));
}
