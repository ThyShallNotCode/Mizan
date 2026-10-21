//! Instructor actions on a graded IAU exam, proven on synthetic sheets only (the committed
//! blank form with synthetic pencil marks):
//! - C5 answer keys: the real key replaces an inferred one (the SCO-003 `inferred_key` blocker
//!   clears) and a mis-keyed entry is corrected, each an audited new key version with its
//!   reason and a full rescore;
//! - C7 override reasons (§13.4, §14.4): overriding the engine's proposal needs a canonical
//!   reason (and a note for `OTHER`), recorded on the audit events and in the history;
//! - C8 the Blackboard Learn grade-center upload: Username and score only, final results only,
//!   never for an inferred key or a rosterless exam.

mod common;

use common::*;
use omr_app::exam::read_key_file;
use omr_app::review::{ReviewReason, reasons_for};
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, ItemKind, KeySource, Lane, RosterStudent,
};
use omr_forms::legacy::LegacyField;
use omr_scoring::AssessmentStatus;
use std::path::Path;

const N: u16 = 29;
const SHEETS: u16 = 6;

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
    format!("22103456{k:02}")
}

fn key_csv(wrong_q3_on_a: bool) -> String {
    let mut s = String::from("question,A,B\r\n");
    for q in 0..N {
        let mut a = key(0, q);
        if wrong_q3_on_a && q == 2 {
            a = (a + 2) % 5;
        }
        s.push_str(&format!(
            "{},{},{}\r\n",
            q + 1,
            char::from(b'a' + a),
            char::from(b'a' + key(1, q))
        ));
    }
    s
}

fn roster() -> Vec<RosterStudent> {
    (0..SHEETS)
        .map(|k| RosterStudent {
            id: student(k),
            name: format!("Student {k}"),
            name_ar: format!("طالب {k}"),
            section: "101".into(),
            numeric_cell: false,
        })
        .collect()
}

/// Six sheets; sheet 2 carries a faint second mark on question 4 (a review item).
fn scans(dir: &Path) -> std::path::PathBuf {
    let e = iau();
    let out = dir.join("scans");
    for k in 0..SHEETS {
        let answers: Vec<Option<u8>> = (0..N).map(|q| answer(k, q)).collect();
        let mut m = Marks::new(&answers, &student(k), Some((k % 2) as u8));
        if k == 2 {
            let o = (answer(k, 3).unwrap() + 2) % 5;
            m.light.push(bubble(&e, LegacyField::Answers, 3, o));
        }
        write(
            &out,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, &m, 90 + u64::from(k))),
        );
    }
    out
}

fn definition(key: KeySource, with_roster: bool) -> ExamDefinition {
    let mut d = legacy_def("Final", N, &["A", "B"], key);
    d.course_code = "CS 522".into();
    if with_roster {
        d.roster = Some(roster());
    }
    d.exam_id = d.derived_exam_id();
    d.validate().unwrap();
    d
}

fn provided(wrong: bool) -> KeySource {
    let forms = vec!["A".to_string(), "B".to_string()];
    let (k, _) = read_key_file(key_csv(wrong).as_bytes(), &forms, N, 5).unwrap();
    KeySource::Provided { forms: k }
}

fn sheet_no(p: &ExamProject, bi: usize) -> u16 {
    source_of(p, bi)[6..8].parse().unwrap()
}

/// Resolves every open item with the truth (reasons where it overrides the engine).
fn resolve_all(p: &mut ExamProject) {
    let open: Vec<(u64, ItemKind, Option<usize>, Option<u16>)> = p
        .queue(None)
        .iter()
        .map(|i| (i.id, i.kind, i.booklet, i.question))
        .collect();
    for (id, kind, b, q) in open {
        let d = match kind {
            ItemKind::Mark => answer(sheet_no(p, b.unwrap()), q.unwrap())
                .map_or(Decision::Blank, Decision::Option),
            ItemKind::Identity => Decision::Student(student(sheet_no(p, b.unwrap()))),
            ItemKind::Form => Decision::Form((sheet_no(p, b.unwrap()) % 2) as u8),
            ItemKind::Notice => Decision::Acknowledge,
        };
        let reason = p
            .overrides_engine(id, &d)
            .then(|| ReviewReason::code("AMBIGUOUS_MARK"));
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

fn hand(k: u16, key_of: &dyn Fn(u16, u16) -> u8) -> String {
    (0..N)
        .filter(|&q| answer(k, q) == Some(key_of(k % 2, q)))
        .count()
        .to_string()
}

fn raw_of(p: &ExamProject, bi: usize) -> (AssessmentStatus, String) {
    let sc = p.scoring.as_ref().unwrap();
    let r = &sc.run.as_ref().unwrap().students[sc.row_of[bi].unwrap()];
    (r.status, r.raw.to_string())
}

fn events(p: &ExamProject) -> Vec<(String, Option<String>, Option<String>)> {
    p.store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .map(|e| (e.event_type, e.reason_code, e.reason_note_ref))
        .collect()
}

#[test]
fn c5_the_real_key_replaces_an_inferred_key_and_the_blocker_clears() {
    let dir = tempfile::tempdir().unwrap();
    let input = scans(dir.path());
    let out = dir.path().join("out");
    let mut p = grade(&definition(KeySource::Inferred, true), &[input], &out, 2);
    // the block and the notice name the remedy: an inferred key is replaceable, never final
    let blocked = p.session.release_blocked().unwrap();
    assert!(blocked.contains("INFERRED"), "{blocked}");
    assert!(
        blocked.contains("Replace key") && !blocked.contains("never"),
        "{blocked}"
    );
    let detail = p
        .session
        .exam_exceptions
        .iter()
        .find(|e| e.sub_reason.as_deref() == Some("inferred_key"))
        .map(|e| e.detail.clone())
        .expect("the inferred-key notice");
    assert!(
        detail.contains("Replace key") && !detail.contains("never"),
        "{detail}"
    );
    assert!(out.join("results-INFERRED.csv").exists());
    let sco003 = p
        .queue(Some(Lane::Scoring))
        .iter()
        .find(|i| i.code == "SCO-003")
        .map(|i| i.id)
        .expect("SCO-003 inferred_key queued");
    // The instructor brings the key file (lowercase letters, CRLF).
    let def = &p.session.definition;
    let (forms, points) = read_key_file(
        key_csv(false).as_bytes(),
        &def.forms,
        def.questions,
        def.options,
    )
    .unwrap();
    p.replace_key(forms, points, None).unwrap();
    let k = p.session.key.as_ref().unwrap();
    assert!(!k.inferred);
    assert_eq!(k.version, 2);
    assert!(!p.session.review.item(sco003).unwrap().is_open());
    assert!(
        p.session
            .exam_exceptions
            .iter()
            .all(|e| e.code != "SCO-003")
    );
    resolve_all(&mut p);
    p.judge_flags().unwrap();
    assert_eq!(p.session.release_blocked(), None, "nothing blocks any more");
    for bi in 0..p.session.booklets.len() {
        let k = sheet_no(&p, bi);
        assert_eq!(
            raw_of(&p, bi),
            (AssessmentStatus::Scored, hand(k, &key)),
            "sheet {k}"
        );
    }
    let ev = events(&p);
    assert!(
        ev.iter()
            .any(|e| e.0 == "KeyReplaced" && e.1.as_deref() == Some("KEY_ERROR"))
    );
    assert!(
        ev.iter()
            .any(|e| e.0 == "KeyVersionPublished" && e.1.as_deref() == Some("KEY_ERROR"))
    );
    // The outputs are no longer marked INFERRED.
    p.export(&out).unwrap();
    assert!(out.join("results.csv").exists());
    let csv = std::fs::read_to_string(out.join("results.csv")).unwrap();
    assert!(!csv.contains("INFERRED"));
    p.store().unwrap().verify().unwrap();
}

#[test]
fn c5_a_mis_keyed_answer_is_corrected_with_an_audited_regrade() {
    let dir = tempfile::tempdir().unwrap();
    let input = scans(dir.path());
    let out = dir.path().join("out");
    let mut p = grade(&definition(provided(true), true), &[input], &out, 2);
    resolve_all(&mut p);
    let wrong = |v: u16, q: u16| {
        if v == 0 && q == 2 {
            (key(0, 2) + 2) % 5
        } else {
            key(v, q)
        }
    };
    for bi in 0..p.session.booklets.len() {
        let k = sheet_no(&p, bi);
        assert_eq!(raw_of(&p, bi).1, hand(k, &wrong), "sheet {k} before");
    }
    // Item analysis would flag it; the instructor corrects form A question 3.
    let right = char::from(b'A' + key(0, 2)).to_string();
    p.set_key_with_reason(
        0,
        3,
        &right,
        Some(ReviewReason::with_note("KEY_ERROR", "typo")),
    )
    .unwrap();
    assert_eq!(p.session.key.as_ref().unwrap().version, 2);
    for bi in 0..p.session.booklets.len() {
        let k = sheet_no(&p, bi);
        assert_eq!(
            raw_of(&p, bi),
            (AssessmentStatus::Scored, hand(k, &key)),
            "sheet {k} after"
        );
    }
    let ev = events(&p);
    let change = ev.iter().find(|e| e.0 == "KeyEntryChanged").unwrap();
    assert_eq!(change.1.as_deref(), Some("KEY_ERROR"));
    // The note never enters the chain in plain text, only a reference to it.
    assert!(change.2.as_deref().unwrap().starts_with("note:"));
    assert!(ev.iter().any(|e| e.0 == "KeyVersionPublished"));
    p.store().unwrap().verify().unwrap();
}

#[test]
fn c7_overriding_the_engine_needs_a_canonical_reason() {
    let dir = tempfile::tempdir().unwrap();
    let input = scans(dir.path());
    let mut p = grade(
        &definition(provided(false), true),
        &[input],
        &dir.path().join("out"),
        2,
    );
    let item = p
        .queue(Some(Lane::Marks))
        .iter()
        .find(|i| i.question == Some(3))
        .map(|i| (*i).clone())
        .expect("the faint second mark is reviewed");
    let sugg = item.suggestion.clone().expect("a proposal");
    let other = match &sugg {
        Decision::Option(o) => Decision::Option((o + 1) % 5),
        _ => Decision::Option(0),
    };
    let resolve = |d: &Decision| Action::Resolve {
        decision: d.clone(),
        accepted_suggestion: false,
        note: String::new(),
    };
    // No reason: refused, and the message says what to give.
    assert!(p.overrides_engine(item.id, &other));
    let e = p.apply(item.id, resolve(&other)).unwrap_err();
    assert_eq!(e.exit_code(), 6);
    assert!(
        e.to_string().contains("reason") && e.to_string().contains("AMBIGUOUS_MARK"),
        "{e}"
    );
    // Unknown codes and OTHER without a note are refused too.
    let e = p
        .apply_with_reason(item.id, resolve(&other), Some(ReviewReason::code("TYPO")))
        .unwrap_err();
    assert!(e.to_string().contains("not a reason code"), "{e}");
    let e = p
        .apply_with_reason(item.id, resolve(&other), Some(ReviewReason::code("OTHER")))
        .unwrap_err();
    assert!(e.to_string().contains("needs a note"), "{e}");
    assert!(p.session.review.item(item.id).unwrap().is_open());
    // With a reason it is recorded: history, view-model and the audit chain (note by hash).
    p.apply_with_reason(
        item.id,
        resolve(&other),
        Some(ReviewReason::with_note(
            "ERASURE_RESIDUE",
            "second mark rubbed out",
        )),
    )
    .unwrap();
    let rec = p.session.review.history.last().unwrap();
    assert_eq!(rec.reason.as_ref().unwrap().code, "ERASURE_RESIDUE");
    assert!(
        p.history_vm()
            .last()
            .unwrap()
            .action
            .ends_with("(ERASURE_RESIDUE)")
    );
    let ev = events(&p);
    let corr = ev
        .iter()
        .rev()
        .find(|e| e.0 == "ResponseCorrected")
        .unwrap();
    assert_eq!(corr.1.as_deref(), Some("ERASURE_RESIDUE"));
    // SEC-08: the note is kept in the store under a random reference; the event carries the
    // reference and a salted digest, never a hash a dictionary of note texts would reverse.
    let note_ref = corr.2.as_deref().unwrap();
    let (r, digest) = note_ref
        .strip_prefix("note:")
        .and_then(|x| x.split_once("#sha384:"))
        .expect("note:<ref>#sha384:<digest>");
    let unsalted = omr_store::jcs::sha384_hex(b"second mark rubbed out");
    assert_ne!(digest, &unsalted[..32]);
    assert_eq!(
        p.store().unwrap().reader().note(r).unwrap().as_deref(),
        Some("second mark rubbed out")
    );
    assert!(
        ev.iter()
            .all(|e| e.2.as_deref() != Some("second mark rubbed out"))
    );
    // Accepting the proposal needs no reason (the item's default code is recorded).
    p.undo().unwrap();
    p.apply(item.id, resolve(&sugg)).unwrap();
    let ev = events(&p);
    assert_eq!(
        ev.iter()
            .rev()
            .find(|e| e.0 == "ResponseCorrected")
            .unwrap()
            .1
            .as_deref(),
        Some("AMBIGUOUS_MARK")
    );
    // The GUI's reason select: the item's family first.
    let codes = reasons_for(ItemKind::Mark, Lane::Marks);
    assert_eq!(codes[0], "AMBIGUOUS_MARK");
    assert!(codes.contains(&"OTHER"));
    assert_eq!(
        reasons_for(ItemKind::Identity, Lane::Identity)[0],
        "ID_CORRECTION"
    );
    p.store().unwrap().verify().unwrap();
}

#[test]
fn c8_blackboard_grade_center_upload_holds_only_what_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let input = scans(dir.path());
    let out = dir.path().join("out");
    let mut p = grade(
        &definition(provided(false), true),
        std::slice::from_ref(&input),
        &out,
        2,
    );
    // Nothing is uploaded before the instructor releases (CR-07), and release is refused while
    // an item is open, with the reason and what to do.
    assert!(!p.queue(None).is_empty());
    assert!(!out.join("blackboard_grade_center.csv").exists());
    let e = p.release(None).unwrap_err().to_string();
    assert!(
        e.contains("release refused") && e.contains("omr review list"),
        "{e}"
    );
    assert!(
        p.export_blackboard(&dir.path().join("early.csv"), None)
            .is_err()
    );
    resolve_all(&mut p);
    p.release(None).unwrap();
    assert!(p.session.released());
    let released = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "ResultsReleased")
        .count();
    assert_eq!(released, 1);
    p.export(&out).unwrap();
    let bytes = std::fs::read(out.join("blackboard_grade_center.csv")).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "UTF-8 BOM");
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines[0],
        "\"Username\",\"CS 522 Final [Total Pts: 29 Score]\""
    );
    assert_eq!(lines.len(), 1 + usize::from(SHEETS));
    let mut want: Vec<String> = (0..SHEETS)
        .map(|k| format!("\"{}\",\"{}\"", student(k), hand(k, &key)))
        .collect();
    want.sort();
    assert_eq!(&lines[1..], want.as_slice());
    // Nothing beyond Username and score: no names (Latin or Arabic), no sections.
    assert!(!text.contains("Student") && !text.contains("طالب") && !text.contains("101"));
    // Listed in the SHA-384 manifest, which carries the Restricted label (§16.3).
    let manifest = std::fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(manifest.contains("blackboard_grade_center.csv"));
    // An existing grade column by its ID; a malformed ID says where to find the right one.
    let f = dir.path().join("bb-col.csv");
    p.export_blackboard(&f, Some("835932")).unwrap();
    let col = std::fs::read_to_string(&f).unwrap();
    assert!(col.contains("\"CS 522 Final [Total Pts: 29 Score] |835932\""));
    let e = p.export_blackboard(&f, Some("83/59")).unwrap_err();
    assert!(e.to_string().contains("Grade Center download"), "{e}");
    // A change after the release supersedes it (§10.8): no upload until released again.
    p.set_key(0, 1, "*").unwrap();
    assert!(!p.session.released());
    assert!(p.export_blackboard(&f, None).is_err());
    // the export removes the upload of the superseded release (and a column upload beside it),
    // so no outdated grades are left to upload
    std::fs::write(
        out.join("blackboard_grade_center-column-835932.csv"),
        b"old",
    )
    .unwrap();
    p.export(&out).unwrap();
    assert!(!out.join("blackboard_grade_center.csv").exists());
    assert!(
        !out.join("blackboard_grade_center-column-835932.csv")
            .exists()
    );
    p.release(None).unwrap();
    p.export(&out).unwrap();
    assert!(out.join("blackboard_grade_center.csv").exists());
    p.close().unwrap();
    // Never for an inferred key, never for a rosterless exam.
    let q = grade(
        &definition(KeySource::Inferred, true),
        std::slice::from_ref(&input),
        &dir.path().join("inf"),
        2,
    );
    assert!(
        !dir.path()
            .join("inf")
            .join("blackboard_grade_center.csv")
            .exists()
    );
    drop(q);
    let mut r = grade(
        &definition(provided(false), false),
        &[input],
        &dir.path().join("nr"),
        2,
    );
    assert!(
        !dir.path()
            .join("nr")
            .join("blackboard_grade_center.csv")
            .exists()
    );
    assert!(
        r.export_blackboard(&dir.path().join("x.csv"), None)
            .is_err()
    );
}

#[test]
fn an_export_file_that_cannot_be_written_is_named_with_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let input = scans(dir.path());
    let out = dir.path().join("out");
    let mut p = grade(
        &definition(provided(false), true),
        std::slice::from_ref(&input),
        &out,
        2,
    );
    // results.csv held where it cannot be replaced (a folder stands in for a file Excel
    // keeps open on Windows)
    std::fs::remove_file(out.join("results.csv")).unwrap();
    std::fs::create_dir(out.join("results.csv")).unwrap();
    let e = p.export(&out).unwrap_err();
    assert_eq!(e.exit_code(), 8, "still a file error");
    let msg = e.to_string();
    assert!(
        msg.contains("results.csv could not be written")
            && msg.contains("if it is open in another program, such as Excel, close it"),
        "{msg}"
    );
}
