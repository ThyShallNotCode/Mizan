//! Key sheets as a key source (spec §10.1, CR-11), proven on synthetic sheets only: the
//! committed blank IAU form with synthetic pencil marks and a synthetic key.
//! - key sheets inside the students' stack (student number bubbled all 0 for version A, all 9
//!   for version B) are found by the rule, taken out of the student population and out of the
//!   students' batch statistics; a student whose number starts with zeros stays a student;
//! - the run is held (SCO-003 BLOCKER `key_sheet_unconfirmed`), nothing is scored and nothing
//!   is releasable until the instructor confirms; the confirmation publishes an audited key
//!   version naming its source, the scans and the operator;
//! - after confirmation every decision file is byte-identical to grading with the same key as
//!   a key file;
//! - rows that are not one confidently read option (several marks, blank, faint) block the
//!   confirmation until decided; a named key sheet that shows no marker (as a sheet registered
//!   one bubble row off would) is refused, never read as a key, and only a key file lifts the
//!   hold then.

mod common;

use common::*;
use omr_app::keysheet::{RowStatus, read_key_sheets, summary_html};
use omr_app::model::{KeyRowDecision, PageStatus, ResponseStatus};
use omr_app::{ExamProject, FormKey, ItemKind, KeySource};
use omr_forms::legacy::LegacyField;
use omr_imaging::GrayImage;
use omr_scoring::AssessmentStatus;
use std::path::{Path, PathBuf};

const N: u16 = 29;

/// The synthetic key of version `v` (0 = A).
fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v * 2) % 5) as u8
}

/// What student `k` (version `v`) marks: wrong on every (k + 3)-th question, a few blanks.
fn answer(k: u16, v: u16, q: u16) -> Option<u8> {
    if (q + k) % 13 == 5 {
        None
    } else if q.is_multiple_of(k + 3) {
        Some((key(v, q) + 1) % 5)
    } else {
        Some(key(v, q))
    }
}

fn hand_score(k: u16, v: u16) -> usize {
    (0..N)
        .filter(|&q| answer(k, v, q) == Some(key(v, q)))
        .count()
}

fn key_marks(v: u16, marker: char) -> Marks {
    let answers: Vec<Option<u8>> = (0..N).map(|q| Some(key(v, q))).collect();
    let id: String = std::iter::repeat_n(marker, 10).collect();
    Marks::new(&answers, &id, Some(v as u8))
}

/// Darkens `bubbles` faintly (a fraction `f` of a solid fill's darkening).
fn faint(e: &omr_recog::LegacyEngine, img: &mut GrayImage, bubbles: &[usize], f: f64, seed: u64) {
    let marked = render(e, bubbles, seed);
    let plain = render(e, &[], seed);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let (m, p) = (marked.get(x, y), plain.get(x, y));
            if m < p {
                let v = f64::from(img.get(x, y)) * (255.0 - f64::from(p - m) * f) / 255.0;
                img.set(x, y, v as u8);
            }
        }
    }
}

struct Stack {
    students: Vec<PathBuf>,
    keys: Vec<PathBuf>,
}

/// Six students (A, B alternating; one leaves the version blank; one bubbles a number that
/// starts with zeros) and the two key sheets (A: 0000000000, B: 9999999999), as PNG files.
fn stack(dir: &Path) -> Stack {
    let e = iau();
    let mut students = Vec::new();
    for k in 0..6u16 {
        let v = k % 2;
        let answers: Vec<Option<u8>> = (0..N).map(|q| answer(k, v, q)).collect();
        let id = if k == 4 {
            "0000012345".to_string()
        } else {
            format!("22104500{k:02}")
        };
        let version = (k != 3).then_some(v as u8);
        let img = sheet(&e, &Marks::new(&answers, &id, version), 100 + u64::from(k));
        students.push(write(dir, &format!("student-{k}.png"), &png(&img)));
    }
    let keys = vec![
        write(dir, "key-A.png", &png(&sheet(&e, &key_marks(0, '0'), 7))),
        write(dir, "key-B.png", &png(&sheet(&e, &key_marks(1, '9'), 8))),
    ];
    Stack { students, keys }
}

fn file_key() -> KeySource {
    KeySource::Provided {
        forms: (0..2u16)
            .map(|v| FormKey {
                form: ["A", "B"][usize::from(v)].into(),
                answers: (0..N)
                    .map(|q| char::from(b'A' + key(v, q)).to_string())
                    .collect(),
            })
            .collect(),
    }
}

const DECISION_FILES: &[&str] = &[
    "results.csv",
    "item_analysis.csv",
    "test_stats.csv",
    "tp154_a1.csv",
    "booklets.json",
    "review_queue.json",
    "key.csv",
];

#[test]
fn key_sheets_in_the_stack_hold_the_run_until_confirmed_then_equal_a_key_file() {
    let tmp = tempfile::tempdir().unwrap();
    let s = stack(&tmp.path().join("scans"));
    let mut inputs = s.students.clone();
    inputs.extend(s.keys.iter().cloned());
    // Graded with no key at all: the key sheets in the stack are the key.
    let def = legacy_def("Final", N, &["A", "B"], KeySource::None);
    let out = tmp.path().join("from-sheets");
    let mut p = grade(&def, &inputs, &out, 2);
    {
        let ss = &p.session;
        let key_pages = ss
            .pages
            .iter()
            .filter(|pg| pg.status == PageStatus::KeySheet)
            .count();
        assert_eq!(key_pages, 2, "both key sheets found in the stack");
        assert_eq!(ss.key_sheets.sheets.len(), 2);
        assert!(
            ss.key_sheets
                .sheets
                .iter()
                .all(|k| !k.declared && k.usable())
        );
        let markers: Vec<Option<u8>> = ss.key_sheets.sheets.iter().map(|k| k.marker).collect();
        assert!(markers.contains(&Some(0)) && markers.contains(&Some(9)));
        // Six students, the one whose number starts with zeros among them.
        assert_eq!(ss.booklets.len(), 6);
        assert!(ss.booklets.iter().any(|b| b.identity.read == "0000012345"));
        // Held: nothing scored, not releasable, one BLOCKER that no queue item can lift.
        assert!(ss.key_unconfirmed());
        assert!(p.scoring.is_none());
        let why = ss.release_blocked().unwrap();
        assert!(
            why.contains("SCO-003") && why.contains("omr key confirm"),
            "{why}"
        );
        assert!(ss.exam_exceptions.iter().any(|e| {
            e.code == "SCO-003"
                && e.severity == omr_model::Severity::Blocker
                && e.sub_reason.as_deref() == Some(omr_app::keysheet::HOLD)
        }));
        assert!(!ss.review.items.iter().any(|i| i.code == "SCO-003"));
        assert!(!out.join("results.csv").exists() && !out.join("key.csv").exists());
        assert!(out.join(omr_app::keysheet::SUMMARY_FILE).exists());
    }
    // The summary: every row read, crops rectified, nothing highlighted.
    let sum = p.key_sheet_summary(true).unwrap();
    assert_eq!(sum.versions.len(), 2);
    for v in &sum.versions {
        assert_eq!(v.rows.len(), usize::from(N));
        assert_eq!(v.open, 0, "version {}", v.version);
        assert!(v.can_confirm);
        assert!(
            v.rows
                .iter()
                .all(|r| r.status == RowStatus::Read && !r.highlight)
        );
        assert!(
            v.rows
                .iter()
                .all(|r| r.crop.as_ref().is_some_and(|c| c.bubbles.len() == 5))
        );
        let f = usize::from(v.version == "B");
        for r in &v.rows {
            let q = r.question as u16 - 1;
            assert_eq!(r.answer, char::from(b'A' + key(f as u16, q)).to_string());
        }
    }
    let html = summary_html(&sum, "omr key confirm --project x");
    assert!(html.contains("Not confirmed yet") && html.contains("data:image/png;base64,"));
    // Never scored from the unconfirmed key: a key entry cannot be changed around it.
    assert!(p.set_key(0, 1, "B").is_err());
    // A confirmation of one version leaves the run held.
    let r = p.confirm_key_sheets(Some(0), Vec::new(), None).unwrap();
    assert_eq!(r.confirmed, vec!["A".to_string()]);
    assert!(r.held && r.key_version.is_none() && p.scoring.is_none());
    // The second lifts the hold: key version 1, published with its provenance.
    p.set_actor("principal:dr-key", "Instructor");
    let r = p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    assert_eq!(r.confirmed, vec!["B".to_string()]);
    assert!(!r.held);
    assert_eq!(r.key_version, Some(1));
    assert!(!p.session.key_unconfirmed());
    assert!(
        p.session
            .release_blocked()
            .is_none_or(|w| !w.contains("SCO-003"))
    );
    let ev = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "KeyVersionPublished")
        .collect::<Vec<_>>();
    assert_eq!(ev.len(), 1);
    let pl = &ev[0].payload;
    assert_eq!(pl["source"], "key_sheet");
    assert_eq!(ev[0].actor_id, "principal:dr-key");
    let mut scans: Vec<String> = pl["scans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["sha384"].as_str().unwrap().to_string())
        .collect();
    scans.sort();
    let mut want: Vec<String> = s
        .keys
        .iter()
        .map(|k| omr_store::jcs::sha384_hex(&std::fs::read(k).unwrap()))
        .collect();
    want.sort();
    assert_eq!(scans, want);
    // Scored: every student's raw score equals the hand count (the blank-version sheet's form
    // is inferred under the confirmed key).
    let sc = p.scoring.as_ref().unwrap();
    let run = sc.run.as_ref().unwrap();
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let name = source_of(&p, bi);
        let k: u16 = name
            .trim_start_matches("student-")
            .trim_end_matches(".png")
            .parse()
            .unwrap();
        assert_eq!(b.form, Some(usize::from(k % 2)), "{name}");
        let row = sc.row_of[bi].unwrap();
        assert_eq!(
            run.students[row].raw,
            omr_scoring::exact(hand_score(k, k % 2) as i128, 1),
            "{name}"
        );
        assert!(b.questions.iter().all(|q| q.status == ResponseStatus::Auto));
    }
    p.export(&out).unwrap();
    // Reopened from the store: the confirmed key verifies against its published version.
    p.close().unwrap();
    let p = ExamProject::open(&out.join("project.omrproj"), "principal:test").unwrap();
    assert!(!p.session.key_unconfirmed());
    p.close().unwrap();
    // The same students graded with the same key as a key file: identical decision files.
    let def2 = legacy_def("Final", N, &["A", "B"], file_key());
    let out2 = tmp.path().join("from-file");
    let q = grade(&def2, &s.students, &out2, 2);
    q.close().unwrap();
    for f in DECISION_FILES {
        let a = std::fs::read(out.join(f)).unwrap();
        let b = std::fs::read(out2.join(f)).unwrap();
        if a != b {
            let (ta, tb) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
            let first = ta
                .lines()
                .zip(tb.lines())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            panic!(
                "{f} differs between the key-sheet run and the key-file run at line {}: {:?} vs {:?}",
                first + 1,
                ta.lines().nth(first),
                tb.lines().nth(first)
            );
        }
    }
}

#[test]
fn rows_not_read_as_one_option_block_confirmation_until_decided() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    let mut m = key_marks(0, '0');
    // Q3: two marks; Q5: no mark; Q8: a faint touch only.
    m.answers[2] = vec![key(0, 2), (key(0, 2) + 2) % 5];
    m.answers[4].clear();
    m.answers[7].clear();
    let mut img = sheet(&e, &m, 21);
    faint(
        &e,
        &mut img,
        &[bubble(&e, LegacyField::Answers, 7, key(0, 7))],
        0.22,
        21,
    );
    let k = write(&dir, "key-A.png", &png(&img));
    let st: Vec<PathBuf> = (0..4u16)
        .map(|j| {
            let answers: Vec<Option<u8>> = (0..N).map(|q| answer(j, 0, q)).collect();
            let img = sheet(
                &e,
                &Marks::new(&answers, &format!("22104511{j:02}"), Some(0)),
                40 + u64::from(j),
            );
            write(&dir, &format!("s{j}.png"), &png(&img))
        })
        .collect();
    let def = legacy_def("Quiz", N, &["A"], KeySource::None);
    let out = tmp.path().join("exam");
    let mut p = ExamProject::grade_with_key_sheets(
        &def,
        &st,
        std::slice::from_ref(&k),
        &options(&out, 2),
        &|_| {},
    )
    .unwrap();
    let ks = &p.session.key_sheets;
    assert_eq!(ks.sheets.len(), 1);
    assert!(ks.sheets[0].declared && ks.sheets[0].usable());
    let sum = p.key_sheet_summary(true).unwrap();
    let v = &sum.versions[0];
    let open: Vec<(u32, RowStatus)> = v
        .rows
        .iter()
        .filter(|r| r.status.open())
        .map(|r| (r.question, r.status))
        .collect();
    assert_eq!(open.len(), 3, "{open:?}");
    assert_eq!(open[0], (3, RowStatus::MultiMark));
    assert_eq!(open[1], (5, RowStatus::Blank));
    assert_eq!(open[2].0, 8);
    assert!(v.rows.iter().filter(|r| r.highlight).count() == 3);
    assert!(!v.can_confirm);
    // The key file of `omr key read` leaves the undecided rows empty (never guessed).
    let csv = p.key_sheet_csv().unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "question,A,points");
    assert_eq!(lines[3], "3,,1");
    assert_eq!(lines[5], "5,,1");
    assert_eq!(lines[8], "8,,1");
    // Confirmation is refused, naming each row.
    let err = p
        .confirm_key_sheets(None, Vec::new(), None)
        .unwrap_err()
        .to_string();
    for q in ["question 3", "question 5", "question 8"] {
        assert!(err.contains(q), "{err}");
    }
    assert!(p.session.key_unconfirmed());
    // Decided: Q3 accepts both marks explicitly, Q5 is voided, Q8 is the faint mark's option.
    let both = format!(
        "{}|{}",
        char::from(b'A' + key(0, 2)),
        char::from(b'A' + (key(0, 2) + 2) % 5)
    );
    let d = |q: usize, a: &str| KeyRowDecision {
        form: 0,
        question: q,
        answer: a.into(),
    };
    let r = p
        .confirm_key_sheets(
            None,
            vec![
                d(2, &both),
                d(4, "*"),
                d(7, &char::from(b'A' + key(0, 7)).to_string()),
            ],
            None,
        )
        .unwrap();
    assert_eq!(r.key_version, Some(1));
    let k = p.session.key.as_ref().unwrap();
    assert_eq!(k.forms[0][4], omr_app::model::KeyCell::Void);
    assert_eq!(k.forms[0][2].text(), both);
    let run = p.scoring.as_ref().unwrap().run.as_ref().unwrap();
    assert!(
        run.students
            .iter()
            .all(|s| s.status != AssessmentStatus::NotScorable)
    );
    p.close().unwrap();
}

#[test]
fn a_named_key_sheet_without_a_marker_is_refused_never_read() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    // What a key sheet registered one bubble row off reads: 1111111111.
    let img = sheet(&e, &key_marks(0, '1'), 31);
    let k = write(&dir, "key-A.png", &png(&img));
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(1, 0, q)).collect();
    let s0 = write(
        &dir,
        "s0.png",
        &png(&sheet(&e, &Marks::new(&answers, "2210450011", Some(0)), 32)),
    );
    // On its own (omr key read): refused, no key.
    let def = legacy_def("Quiz", N, &["A"], KeySource::None);
    let r = read_key_sheets(&def, std::slice::from_ref(&k), 2).unwrap();
    let sum = r.key_sheet_summary(false).unwrap();
    assert!(sum.versions.is_empty());
    assert_eq!(sum.unusable.len(), 1);
    assert!(
        sum.unusable[0]
            .refusal
            .as_ref()
            .unwrap()
            .contains("1111111111")
    );
    assert!(r.key_sheet_csv().is_none());
    // In an exam: never a student, the run held, nothing to confirm; a key file lifts it.
    let out = tmp.path().join("exam");
    let mut p = ExamProject::grade_with_key_sheets(
        &def,
        std::slice::from_ref(&s0),
        std::slice::from_ref(&k),
        &options(&out, 1),
        &|_| {},
    )
    .unwrap();
    assert_eq!(p.session.booklets.len(), 1);
    assert!(p.session.key_unconfirmed() && p.scoring.is_none());
    assert!(p.confirm_key_sheets(None, Vec::new(), None).is_err());
    let fk = FormKey {
        form: "A".into(),
        answers: (0..N)
            .map(|q| char::from(b'A' + key(0, q)).to_string())
            .collect(),
    };
    p.replace_key(vec![fk], None, None).unwrap();
    assert!(!p.session.key_unconfirmed());
    assert!(p.session.key_sheets.superseded);
    let run = p.scoring.as_ref().unwrap().run.as_ref().unwrap();
    assert_eq!(
        run.students[0].raw,
        omr_scoring::exact(hand_score(1, 0) as i128, 1)
    );
    assert!(
        !p.session
            .review
            .items
            .iter()
            .any(|i| i.kind == ItemKind::Notice && i.code == "SCO-003")
    );
    p.close().unwrap();
}

#[test]
fn a_confirmed_key_sheet_stays_confirmed_when_scans_are_added() {
    let tmp = tempfile::tempdir().unwrap();
    let s = stack(&tmp.path().join("scans"));
    let def = legacy_def("Final", N, &["A", "B"], KeySource::None);
    let out = tmp.path().join("exam");
    let mut first = s.students[..3].to_vec();
    first.extend(s.keys.iter().cloned());
    let mut p = grade(&def, &first, &out, 2);
    // Adding scans waits for the confirmation.
    assert!(
        p.add_scans(&s.students[3..], &options(&out, 2), &|_| {})
            .is_err()
    );
    p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    let r = p
        .add_scans(&s.students[3..], &options(&out, 2), &|_| {})
        .unwrap();
    assert_eq!(r.booklets_after, 6);
    assert!(!p.session.key_unconfirmed());
    assert_eq!(p.session.key.as_ref().unwrap().version, 1);
    assert_eq!(p.session.key_sheets.sheets.len(), 2);
    assert!(p.scoring.as_ref().unwrap().run.is_ok());
    p.close().unwrap();
}

/// Adversarial verification (key sheets): a key sheet in the stack whose version bubble was
/// left blank is still a key sheet. On an exam of several versions it is refused with what to
/// do and holds the run; on a one-version exam it is that version. It is never graded as a
/// student whose number is the marker (it was: a booklet `0000000000` with no form, whose
/// answers entered the students' statistics and an inferred key).
#[test]
fn a_key_sheet_with_a_blank_version_is_never_graded_as_a_student() {
    let e = iau();
    for forms in [vec!["A", "B"], vec!["A"]] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scans");
        let mut m = key_marks(0, '0');
        m.version = None;
        let mut inputs: Vec<PathBuf> = (0..4u16)
            .map(|k| {
                let v = k % forms.len() as u16;
                let answers: Vec<Option<u8>> = (0..N).map(|q| answer(k, v, q)).collect();
                let img = sheet(
                    &e,
                    &Marks::new(&answers, &format!("22104500{k:02}"), Some(v as u8)),
                    60 + u64::from(k),
                );
                write(&dir, &format!("s{k}.png"), &png(&img))
            })
            .collect();
        inputs.insert(2, write(&dir, "key.png", &png(&sheet(&e, &m, 61))));
        let def = legacy_def("Final", N, &forms, KeySource::None);
        let out = tmp.path().join("exam");
        let p = grade(&def, &inputs, &out, 2);
        let ss = &p.session;
        assert_eq!(
            ss.booklets.len(),
            4,
            "forms {forms:?}: the key sheet is no booklet"
        );
        assert!(ss.booklets.iter().all(|b| b.identity.read != "0000000000"));
        assert_eq!(ss.key_sheets.sheets.len(), 1);
        let k = &ss.key_sheets.sheets[0];
        assert_eq!(k.marker, Some(0));
        assert!(ss.key_unconfirmed() && p.scoring.is_none());
        if forms.len() > 1 {
            let why = k.refusal.as_deref().unwrap_or_default();
            assert!(
                why.contains("version is not bubbled") && why.contains("--key-sheet-marker none"),
                "{why}"
            );
            assert!(!k.usable());
            let sum = p.key_sheet_summary(false).unwrap();
            assert!(sum.versions.is_empty() && sum.unusable.len() == 1);
        } else {
            assert!(k.usable() && k.form == Some(0), "{:?}", k.refusal);
        }
        p.close().unwrap();
    }
}

/// Adversarial verification (key sheets): an answer the instructor erased and changed on the
/// key sheet reads as the new answer, and the row is highlighted for checking (MRK-003), as is a
/// row where a key file given too says otherwise; neither blocks the confirmation.
#[test]
fn an_erased_and_changed_key_answer_is_highlighted_for_checking() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    let mut img = sheet(&e, &key_marks(0, '0'), 5);
    for q in [5u16, 9] {
        let old = (key(0, q) + 2) % 5;
        faint(
            &e,
            &mut img,
            &[bubble(&e, LegacyField::Answers, q, old)],
            0.3,
            5,
        );
    }
    let k = write(&dir, "key-A.png", &png(&img));
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(1, 0, q)).collect();
    let s0 = write(
        &dir,
        "s0.png",
        &png(&sheet(&e, &Marks::new(&answers, "2210450011", Some(0)), 32)),
    );
    // A key file is given too, and it says otherwise on question 20.
    let mut fk: Vec<String> = (0..N)
        .map(|q| char::from(b'A' + key(0, q)).to_string())
        .collect();
    fk[19] = char::from(b'A' + (key(0, 19) + 1) % 5).to_string();
    let def = legacy_def(
        "Quiz",
        N,
        &["A"],
        KeySource::Provided {
            forms: vec![FormKey {
                form: "A".into(),
                answers: fk,
            }],
        },
    );
    let out = tmp.path().join("exam");
    let p = ExamProject::grade_with_key_sheets(
        &def,
        std::slice::from_ref(&s0),
        std::slice::from_ref(&k),
        &options(&out, 1),
        &|_| {},
    )
    .unwrap();
    let sum = p.key_sheet_summary(false).unwrap();
    let v = &sum.versions[0];
    assert_eq!(v.open, 0);
    assert!(v.can_confirm);
    let hi: Vec<u32> = v
        .rows
        .iter()
        .filter(|r| r.highlight)
        .map(|r| r.question)
        .collect();
    assert_eq!(hi, vec![6, 10, 20], "{:?}", v.rows);
    for q in [6u32, 10] {
        let r = &v.rows[q as usize - 1];
        assert_eq!(r.status, RowStatus::Read);
        assert_eq!(
            r.answer,
            char::from(b'A' + key(0, q as u16 - 1)).to_string()
        );
        assert!(r.detail.contains("MRK-003"), "{}", r.detail);
    }
    assert!(v.rows[19].key_file.is_some());
    // C2: the rendered summary the instructor confirms says why each highlighted row is
    // highlighted, also when it reads one option ("read").
    let html = summary_html(&sum, "omr key confirm --project x");
    let rows: Vec<&str> = html.split("<tr").skip(1).collect();
    let hi: Vec<&&str> = rows
        .iter()
        .filter(|r| r.starts_with(" class=\"hi\""))
        .collect();
    assert_eq!(hi.len(), 3, "{html}");
    for q in [6u32, 10] {
        let row = hi
            .iter()
            .find(|r| r.contains(&format!("<td class=\"q\">{q}</td>")))
            .unwrap_or_else(|| panic!("row {q} highlighted"));
        assert!(row.contains("MRK-003"), "row {q}: {row}");
    }
    assert!(
        hi.iter()
            .find(|r| r.contains("<td class=\"q\">20</td>"))
            .unwrap()
            .contains("the key file says")
    );
    p.close().unwrap();
}

/// Adversarial verification (key sheets): a student who bubbled the marker by mistake is a
/// second key sheet of the version; the hold says the sheets disagree and how to have the
/// student graded, and grading again with the real key sheet named and no marker rule grades
/// the student.
#[test]
fn a_student_who_bubbles_the_marker_is_named_by_the_hold_with_the_way_out() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    let k = write(&dir, "key-A.png", &png(&sheet(&e, &key_marks(0, '0'), 11)));
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(7, 0, q)).collect();
    let zero = write(
        &dir,
        "s-zero.png",
        &png(&sheet(&e, &Marks::new(&answers, "0000000000", Some(0)), 13)),
    );
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(1, 0, q)).collect();
    let s0 = write(
        &dir,
        "s0.png",
        &png(&sheet(&e, &Marks::new(&answers, "2210450011", Some(0)), 32)),
    );
    let students = vec![s0, zero];
    let def = legacy_def("Quiz", N, &["A"], KeySource::None);
    let p = ExamProject::grade_with_key_sheets(
        &def,
        &students,
        std::slice::from_ref(&k),
        &options(&tmp.path().join("exam"), 1),
        &|_| {},
    )
    .unwrap();
    assert_eq!(p.session.key_sheets.sheets.len(), 2);
    let hold = p
        .session
        .exam_exceptions
        .iter()
        .find(|e| e.sub_reason.as_deref() == Some(omr_app::keysheet::HOLD))
        .unwrap();
    assert!(
        hold.detail.contains("disagree") && hold.detail.contains("--key-sheet-marker none"),
        "{}",
        hold.detail
    );
    p.close().unwrap();
    let mut def = def;
    def.key_sheets = omr_app::exam::KeySheetRule::off();
    let mut p = ExamProject::grade_with_key_sheets(
        &def,
        &students,
        std::slice::from_ref(&k),
        &options(&tmp.path().join("exam2"), 1),
        &|_| {},
    )
    .unwrap();
    assert_eq!(p.session.booklets.len(), 2);
    assert_eq!(p.session.key_sheets.sheets.len(), 1);
    p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    assert!(p.scoring.as_ref().unwrap().run.is_ok());
    p.close().unwrap();
}

/// C4: a student who bubbled the marker number (all 0) and answered exactly like the key (a
/// full-marks student) reads as a second key sheet that agrees with the real one. They must not
/// be dropped silently: the hold, the key summary (its notes and page) and the CLI notes name
/// every key sheet of the version, with the way to get the student graded.
#[test]
fn a_marker_student_who_agrees_with_the_key_is_named_never_dropped_silently() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    let k = write(&dir, "key-A.png", &png(&sheet(&e, &key_marks(0, '0'), 11)));
    // Every answer equals the key: the same reading as the key sheet.
    let full: Vec<Option<u8>> = (0..N).map(|q| Some(key(0, q))).collect();
    let top = write(
        &dir,
        "s-top.png",
        &png(&sheet(&e, &Marks::new(&full, "0000000000", Some(0)), 17)),
    );
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(1, 0, q)).collect();
    let s0 = write(
        &dir,
        "s0.png",
        &png(&sheet(&e, &Marks::new(&answers, "2210450011", Some(0)), 32)),
    );
    let def = legacy_def("Quiz", N, &["A"], KeySource::None);
    let out = tmp.path().join("exam");
    // The key sheet in the stack with the students (nothing named).
    let p = grade(&def, &[s0, top, k], &out, 1);
    let ks = &p.session.key_sheets;
    assert_eq!(
        ks.sheets.len(),
        2,
        "the marker student reads as a key sheet"
    );
    assert!(ks.sheets.iter().all(|k| !k.declared && k.usable()));
    assert_eq!(p.session.booklets.len(), 1);
    // They agree on every row: nothing to decide, yet both are named.
    let sum = p.key_sheet_summary(false).unwrap();
    assert_eq!(sum.versions[0].open, 0);
    let hold = p
        .session
        .exam_exceptions
        .iter()
        .find(|e| e.sub_reason.as_deref() == Some(omr_app::keysheet::HOLD))
        .unwrap();
    for name in ["key-A.png page 1", "s-top.png page 1"] {
        assert!(hold.detail.contains(name), "{name}: {}", hold.detail);
        assert!(
            sum.notes.iter().any(|n| n.contains(name)),
            "{name}: {:?}",
            sum.notes
        );
    }
    assert!(
        hold.detail.contains("--key-sheet-marker none"),
        "{}",
        hold.detail
    );
    let html = std::fs::read_to_string(out.join(omr_app::keysheet::SUMMARY_FILE)).unwrap();
    assert!(
        html.contains("s-top.png page 1"),
        "the summary page names it"
    );
    p.close().unwrap();
}

/// C5: each key-summary row crop shows the question number printed beside the row on the
/// sheet, so a key sheet read one row off cannot look right under its question: the label
/// area of question q's crop matches the blank form's label of row q, not of its neighbours.
#[test]
fn a_key_summary_row_crop_shows_its_own_printed_question_number() {
    use omr_app::engine::{BubbleGeom, GroupGeom};
    use omr_app::keysheet::{LABEL_LEAD_PITCHES, rectified_row};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("scans");
    let e = iau();
    let k = write(&dir, "key-A.png", &png(&sheet(&e, &key_marks(0, '0'), 11)));
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(1, 0, q)).collect();
    let s0 = write(
        &dir,
        "s0.png",
        &png(&sheet(&e, &Marks::new(&answers, "2210450011", Some(0)), 32)),
    );
    let def = legacy_def("Quiz", N, &["A"], KeySource::None);
    let p = ExamProject::grade_with_key_sheets(
        &def,
        std::slice::from_ref(&s0),
        std::slice::from_ref(&k),
        &options(&tmp.path().join("exam"), 1),
        &|_| {},
    )
    .unwrap();
    let sum = p.key_sheet_summary(true).unwrap();
    let blank = omr_imaging::decode(omr_forms::legacy::IAU_CCSIT_V1_BLANK_PNG)
        .unwrap()
        .image;
    let r = e.template().bubble_radius_px() as f32;
    let row = |q: u16| GroupGeom {
        bubbles: e
            .template()
            .bubbles()
            .iter()
            .filter(|b| b.field == LegacyField::Answers && b.group == q)
            .map(|b| BubbleGeom {
                option: b.option,
                cx: b.centre.x as f32,
                cy: b.centre.y as f32,
                r,
                fill: 0.0,
                unreadable: false,
            })
            .collect(),
    };
    let labels: Vec<String> = ["A", "B", "C", "D", "E"].map(String::from).to_vec();
    let luma = |c: &omr_app::crops::Crop, x: usize, y: usize| {
        f64::from(c.rgba[(y * c.width as usize + x) * 4])
    };
    for q in [3u16, 12, 24] {
        let c = sum.versions[0].rows[usize::from(q)]
            .crop
            .as_ref()
            .expect("row crop");
        // The label area: everything left of option A's ring.
        let label_w = (c.bubbles[0].cx - 1.5 * c.bubbles[0].r).max(1.0) as usize;
        assert!(
            label_w as f32 > 1.5 * (c.bubbles[1].cx - c.bubbles[0].cx),
            "the crop reaches the printed number: {label_w} px"
        );
        let diff = |qq: u16| {
            let b = rectified_row(&blank, &row(qq), &labels, &[], 2.0, LABEL_LEAD_PITCHES).unwrap();
            let (w, h) = (
                label_w.min(b.width as usize),
                c.height.min(b.height) as usize,
            );
            let mut s = 0.0;
            for y in 0..h {
                for x in 0..w {
                    s += (luma(c, x, y) - luma(&b, x, y)).abs();
                }
            }
            s / (w * h) as f64
        };
        let (own, above, below) = (diff(q), diff(q - 1), diff(q + 1));
        assert!(
            own * 1.5 < above.min(below),
            "Q{}: own {own:.1}, row above {above:.1}, row below {below:.1}",
            q + 1
        );
    }
    let html = summary_html(&sum, "omr key confirm --project x");
    assert!(html.contains("must match the Q column"));
    p.close().unwrap();
}

/// Adversarial verification (key sheets): a key entry corrected after the key-sheet key was
/// confirmed (`omr key set`, an audited new key version) stays the instructor's decision when
/// the key sheet is scanned again in a later batch, which drops the confirmation: the next
/// summary shows the row as decided and the re-confirmed key keeps the correction (it reverted
/// silently to the sheet's reading).
#[test]
fn a_key_correction_after_confirmation_survives_a_rescan_of_the_key_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let s = stack(&tmp.path().join("scans"));
    let def = legacy_def("Final", N, &["A", "B"], KeySource::None);
    let out = tmp.path().join("exam");
    let mut all = s.students.clone();
    all.extend(s.keys.iter().cloned());
    let mut p = grade(&def, &all, &out, 2);
    p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    let fixed = char::from(b'A' + (key(0, 1) + 1) % 5).to_string();
    p.set_key(0, 2, &fixed).unwrap();
    assert_eq!(p.session.key.as_ref().unwrap().version, 2);
    // The key sheet of version A scanned again, in a later batch.
    let e = iau();
    let rescan = write(
        &tmp.path().join("later"),
        "key-A-again.png",
        &png(&sheet(&e, &key_marks(0, '0'), 17)),
    );
    p.add_scans(std::slice::from_ref(&rescan), &options(&out, 2), &|_| {})
        .unwrap();
    assert!(p.session.key_unconfirmed(), "a new key sheet waits again");
    let sum = p.key_sheet_summary(false).unwrap();
    let a = sum.versions.iter().find(|v| v.version == "A").unwrap();
    let row = &a.rows[1];
    assert_eq!(row.status, RowStatus::Decided);
    assert_eq!(row.answer, fixed);
    assert!(row.highlight);
    let r = p.confirm_key_sheets(None, Vec::new(), None).unwrap();
    assert_eq!(r.key_version, Some(3));
    assert_eq!(p.session.key.as_ref().unwrap().forms[0][1].text(), fixed);
    p.close().unwrap();
}
