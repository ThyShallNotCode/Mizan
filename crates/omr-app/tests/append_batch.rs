//! C4: a later batch appended to a graded exam (a second section scanned on another day),
//! proven on synthetic IAU sheets only (the committed blank form with synthetic pencil marks).
//! Grading batch 1 and then appending batch 2 gives decision files byte-identical to grading
//! both at once; review decisions taken in between carry over; the append is audited (new
//! scan batch and runs, renumbered serials); a file imported again is CAP-011 and adds nothing.

mod common;

use common::*;
use omr_app::review::ReviewReason;
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, ItemKind, KeySource, RosterStudent,
};
use omr_forms::legacy::LegacyField;
use std::path::{Path, PathBuf};

const N: u16 = 29;
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

fn student(k: u16) -> String {
    format!("22103457{k:02}")
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
            &png(&sheet(&e, &m, 300 + u64::from(k))),
        );
    }
    dir.to_path_buf()
}

fn definition() -> ExamDefinition {
    let mut d = legacy_def(
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
    );
    d.roster = Some(
        (0..SHEETS)
            .map(|k| RosterStudent {
                id: student(k),
                name: format!("Student {k}"),
                name_ar: String::new(),
                section: if k < 4 { "101" } else { "102" }.into(),
                numeric_cell: false,
            })
            .collect(),
    );
    d.validate().unwrap();
    d
}

const DECISION_FILES: &[&str] = &[
    "results.csv",
    "item_analysis.csv",
    "test_stats.csv",
    "tp154_a1.csv",
    "key.csv",
    "booklets.json",
    "blackboard_grade_center.csv",
];

fn same_decisions(a: &ExamProject, b: &ExamProject, da: &Path, db: &Path) {
    for f in DECISION_FILES {
        let (x, y) = (std::fs::read(da.join(f)), std::fs::read(db.join(f)));
        assert!(x.is_ok() || f.starts_with("blackboard"), "{f} missing");
        assert!(x.ok() == y.ok(), "{f} differs");
    }
    assert_eq!(a.session.pages, b.session.pages);
    assert_eq!(a.session.booklets, b.session.booklets);
    assert_eq!(a.session.key, b.session.key);
    assert_eq!(a.session.review.items.len(), b.session.review.items.len());
    for (x, y) in a.session.review.items.iter().zip(&b.session.review.items) {
        assert_eq!(
            (
                &x.id,
                &x.code,
                &x.booklet,
                &x.question,
                &x.group_key,
                &x.suggestion,
                &x.state
            ),
            (
                &y.id,
                &y.code,
                &y.booklet,
                &y.question,
                &y.group_key,
                &y.suggestion,
                &y.state
            )
        );
    }
}

fn light_item(p: &ExamProject, k: u16) -> u64 {
    p.queue(None)
        .iter()
        .find(|i| {
            i.kind == ItemKind::Mark
                && i.question == Some(3)
                && source_of(p, i.booklet.unwrap()) == format!("sheet-{k:02}.png")
        })
        .map(|i| i.id)
        .expect("light mark reviewed")
}

#[test]
fn c4_appending_a_batch_equals_grading_everything_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let a = sheets(&dir.path().join("day1"), 0..4);
    let b = sheets(&dir.path().join("day2"), 4..SHEETS);
    let def = definition();
    // Everything at once.
    let all_out = dir.path().join("all");
    let all = grade(&def, &[a.clone(), b.clone()], &all_out, 2);
    // Batch 1, then batch 2 appended (another process: the project is reopened).
    let inc_out = dir.path().join("inc");
    let first = grade(&def, std::slice::from_ref(&a), &inc_out, 3);
    assert_eq!(first.session.booklets.len(), 4);
    let first_booklets = first.session.booklets.clone();
    first.close().unwrap();
    let mut inc = ExamProject::open(&inc_out.join("project.omrproj"), "principal:clerk").unwrap();
    let r = inc
        .add_scans(std::slice::from_ref(&b), &options(&inc_out, 1), &|_| {})
        .unwrap();
    inc.export(&inc_out).unwrap();
    assert_eq!(r.batch, "batch-2");
    assert_eq!((r.files, r.new_pages, r.duplicates), (4, 4, 0));
    assert_eq!((r.booklets_before, r.booklets_after), (4, 8));
    same_decisions(&all, &inc, &all_out, &inc_out);
    // Audited: a second batch and runs; serials that moved are logged; the chain verifies.
    let ev: Vec<String> = inc
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .map(|e| e.event_type)
        .collect();
    let count = |t: &str| ev.iter().filter(|e| *e == t).count();
    assert_eq!(count("BatchOpened"), 2);
    assert_eq!(count("BatchClosed"), 2);
    assert_eq!(count("RecognitionRunStarted"), 2);
    assert_eq!(count("IdentityRunStarted"), 2);
    let moved = all
        .session
        .booklets
        .iter()
        .filter(|b| {
            first_booklets
                .iter()
                .any(|f| f.booklet_id == b.booklet_id && f.serial != b.serial)
        })
        .count();
    assert!(moved > 0, "the second day's sheets interleave by content");
    assert_eq!(count("BookletRenumbered"), moved);
    inc.store().unwrap().verify().unwrap();
    // A file imported again: CAP-011, nothing added, nothing queued.
    let again = dir.path().join("again");
    std::fs::create_dir_all(&again).unwrap();
    std::fs::copy(a.join("sheet-02.png"), again.join("sheet-02-copy.png")).unwrap();
    let queue_before = inc.queue(None).len();
    let r = inc
        .add_scans(&[again], &options(&inc_out, 2), &|_| {})
        .unwrap();
    assert_eq!((r.files, r.new_pages, r.duplicates), (1, 0, 1));
    assert_eq!(r.booklets_after, 8);
    assert_eq!(inc.queue(None).len(), queue_before);
    let dup = inc
        .session
        .pages
        .iter()
        .find(|p| inc.session.sources[p.source].name == "sheet-02-copy.png")
        .unwrap();
    assert_eq!(dup.refusal.as_ref().unwrap().code, "CAP-011");
    assert_eq!(dup.booklet, None);
    inc.store().unwrap().verify().unwrap();
}

#[test]
fn c4_review_decisions_taken_before_the_append_carry_over() {
    let dir = tempfile::tempdir().unwrap();
    let a = sheets(&dir.path().join("day1"), 0..4);
    let b = sheets(&dir.path().join("day2"), 4..SHEETS);
    let def = definition();
    let resolve = |p: &mut ExamProject| {
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
    };
    // Reference: everything at once, then the decisions.
    let all_out = dir.path().join("all");
    let mut all = grade(&def, &[a.clone(), b.clone()], &all_out, 2);
    resolve(&mut all);
    all.export(&all_out).unwrap();
    // Day 1 graded and reviewed; day 2 appended later.
    let inc_out = dir.path().join("inc");
    let mut inc = grade(&def, std::slice::from_ref(&a), &inc_out, 2);
    let id = light_item(&inc, 1);
    let truth = Decision::Option(answer(1, 3).unwrap());
    let reason = inc
        .overrides_engine(id, &truth)
        .then(|| ReviewReason::code("ERASURE_RESIDUE"));
    inc.apply_with_reason(
        id,
        Action::Resolve {
            decision: truth,
            accepted_suggestion: false,
            note: String::new(),
        },
        reason,
    )
    .unwrap();
    let r = inc.add_scans(&[b], &options(&inc_out, 2), &|_| {}).unwrap();
    assert_eq!((r.carried_over, r.conflicts), (1, 0));
    inc.apply(light_item(&inc, 6), Action::Defer).unwrap();
    inc.export(&all_out.with_file_name("inc-export")).unwrap();
    let inc_dir = all_out.with_file_name("inc-export");
    for f in [
        "results.csv",
        "booklets.json",
        "key.csv",
        "item_analysis.csv",
    ] {
        assert!(
            std::fs::read(all_out.join(f)).unwrap() == std::fs::read(inc_dir.join(f)).unwrap(),
            "{f} differs"
        );
    }
    assert_eq!(all.session.booklets, inc.session.booklets);
    let states = |p: &ExamProject| -> Vec<_> {
        p.session
            .review
            .items
            .iter()
            .map(|i| (i.id, i.state, i.resolution.clone()))
            .collect()
    };
    assert_eq!(states(&all), states(&inc));
    // The carried decision is in the history but sealed: undo stops at the append.
    inc.undo().unwrap(); // the deferral made after the append
    let e = inc.undo().unwrap_err();
    assert!(e.to_string().contains("appended"), "{e}");
    inc.store().unwrap().verify().unwrap();
}
