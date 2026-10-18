//! What students do on IAU sheets, proven on synthetic sheets only (the committed blank form
//! with synthetic pencil marks):
//! - C9 partial and stray input: an absent (fully blank) sheet is held, never scored 0 (MRK-015);
//!   a sheet answering only some questions scores the rest as blank; marks in the unused rows
//!   30–100 of the 100-question sheet are ignored (the spec asks no code for them); a row with
//!   every bubble filled is a multiple mark, never credited;
//! - C10 ID mistakes: a blank ID column, two bubbles in one column, an ID on no roster entry and
//!   two sheets with one ID give their IDN-004 sub-reason or IDN-005, and land in the Identity
//!   lane with a usable roster suggestion where the roster allows one.

mod common;

use common::*;
use omr_app::model::ResponseStatus;
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, ItemKind, KeySource, Lane,
    RosterStudent,
};
use omr_forms::legacy::LegacyField;
use omr_model::Response;
use omr_scoring::AssessmentStatus;

const N: u16 = 29;

fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

/// The usual answers of sheet `k`: wrong on every (k + 2)-th question, blank on a few.
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

fn answers(k: u16) -> Vec<Option<u8>> {
    (0..N).map(|q| answer(k, q)).collect()
}

fn definition(title: &str, roster: &[String]) -> ExamDefinition {
    let mut d = legacy_def(
        title,
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
        roster
            .iter()
            .enumerate()
            .map(|(i, id)| RosterStudent {
                id: id.clone(),
                name: format!("Student {i}"),
                name_ar: format!("طالب {i}"),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    d.validate().unwrap();
    d
}

/// An identity item: ID, booklet, code, sub-reason, suggestion.
type IdItem = (u64, usize, String, Option<String>, Option<Decision>);

fn booklet_of(p: &ExamProject, k: u16) -> usize {
    (0..p.session.booklets.len())
        .find(|&bi| source_of(p, bi).starts_with(&format!("sheet-{k:02}")))
        .unwrap_or_else(|| panic!("no booklet for sheet {k}"))
}

fn status(p: &ExamProject, bi: usize) -> (AssessmentStatus, String) {
    let sc = p.scoring.as_ref().unwrap();
    let r = &sc.run.as_ref().unwrap().students[sc.row_of[bi].unwrap()];
    (r.status, r.raw.to_string())
}

#[test]
fn c9_absent_partial_stray_and_full_row_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    let roster: Vec<String> = (0..5).map(student).collect();
    // 0: a normal sheet (control).
    let normal = Marks::new(&answers(0), &student(0), Some(0));
    // 1: absent: the sheet came back with nothing on it.
    let absent = Marks::default();
    // 2: answers only questions 1–15.
    let partial_answers: Vec<Option<u8>> = (0..N)
        .map(|q| if q < 15 { answer(2, q) } else { None })
        .collect();
    let partial = Marks::new(&partial_answers, &student(2), Some(0));
    // 3: stray marks in the unused rows 30–100 (one or two bubbles each).
    let mut stray = Marks::new(&answers(3), &student(3), Some(1));
    for q in N..100 {
        stray
            .extra
            .push(bubble(&e, LegacyField::Answers, q, (q % 5) as u8));
        if q % 7 == 0 {
            stray
                .extra
                .push(bubble(&e, LegacyField::Answers, q, ((q + 2) % 5) as u8));
        }
    }
    // 4: every bubble of question 6 filled.
    let mut full = Marks::new(&answers(4), &student(4), Some(0));
    full.answers[5] = (0..5).collect();
    for (k, m) in [normal, absent, partial, stray, full].iter().enumerate() {
        write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, m, 30 + k as u64)),
        );
    }
    let def = definition("Partial input", &roster);
    let p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let s = &p.session;
    assert_eq!(s.booklets.len(), 5);
    let hand = |k: u16, qs: &[u16]| {
        qs.iter()
            .filter(|&&q| answer(k, q) == Some(key(k % 2, q)))
            .count()
            .to_string()
    };
    let all: Vec<u16> = (0..N).collect();

    // Control.
    let b0 = booklet_of(&p, 0);
    assert_eq!(status(&p, b0), (AssessmentStatus::Scored, hand(0, &all)));

    // Absent: one booklet, held as pending (never scored 0), queued as a blank booklet, and
    // its identity is queued too (nothing bubbled).
    let b1 = booklet_of(&p, 1);
    assert_eq!(s.booklets[b1].hold.as_deref(), Some("MRK-015"));
    assert_eq!(status(&p, b1).0, AssessmentStatus::Provisional);
    let items: Vec<_> = p
        .queue(None)
        .into_iter()
        .filter(|i| i.booklet == Some(b1))
        .collect();
    assert!(items.iter().any(|i| i.code == "MRK-015"), "{items:?}");
    assert!(
        items
            .iter()
            .any(|i| i.code == "IDN-004" && i.sub_reason.as_deref() == Some("id_blank")),
        "{:?}",
        items
            .iter()
            .map(|i| (&i.code, &i.sub_reason))
            .collect::<Vec<_>>()
    );

    // Partial: unanswered questions are confident blanks, scored as not answered.
    let b2 = booklet_of(&p, 2);
    for (q, qs) in s.booklets[b2].questions.iter().enumerate() {
        if q >= 15 {
            assert_eq!(
                (qs.status, &qs.current),
                (ResponseStatus::Auto, &Response::Blank),
                "q{q}"
            );
        }
    }
    let first15: Vec<u16> = (0..15).collect();
    assert_eq!(
        status(&p, b2),
        (AssessmentStatus::Scored, hand(2, &first15))
    );

    // Stray marks below the exam's last question: ignored (29 questions read, nothing queued,
    // the score is the hand count of questions 1–29).
    let b3 = booklet_of(&p, 3);
    assert_eq!(s.booklets[b3].questions.len(), usize::from(N));
    assert!(p.queue(None).iter().all(|i| i.booklet != Some(b3)));
    assert_eq!(status(&p, b3), (AssessmentStatus::Scored, hand(3, &all)));

    // Every bubble of a row: a multiple mark (MRK-002), never credited although the keyed
    // option is among the marks.
    let b4 = booklet_of(&p, 4);
    let q5 = &s.booklets[b4].questions[5];
    assert_eq!(q5.recognized.response, Response::Multi(vec![0, 1, 2, 3, 4]));
    assert!(q5.recognized.exceptions.iter().any(|e| e.code == "MRK-002"));
    let others: Vec<u16> = (0..N).filter(|&q| q != 5).collect();
    let (st, raw) = status(&p, b4);
    if q5.status == ResponseStatus::Auto {
        assert_eq!((st, raw), (AssessmentStatus::Scored, hand(4, &others)));
    } else {
        // Reviewed as a multi: still no credit.
        assert_eq!(st, AssessmentStatus::Provisional);
    }
}

#[test]
fn c10_id_mistakes_get_idn_codes_and_roster_suggestions() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    let mut roster: Vec<String> = (0..6).map(student).collect();
    roster.push("2210349999".into());
    let digits = |id: &str| -> Vec<Vec<u8>> { id.bytes().map(|c| vec![c - b'0']).collect() };
    let mut sheets: Vec<Marks> = Vec::new();
    // 0: correct ID (control).
    sheets.push(Marks::new(&answers(0), &student(0), Some(0)));
    // 1: the third column left blank.
    let mut m = Marks::new(&answers(1), &student(1), Some(1));
    m.id = digits(&student(1));
    m.id[2].clear();
    sheets.push(m);
    // 2: two bubbles in the sixth column (the right digit and another).
    let mut m = Marks::new(&answers(2), &student(2), Some(0));
    m.id[5] = vec![4, 7];
    sheets.push(m);
    // 3: an ID on no roster entry (one digit off the student's own).
    sheets.push(Marks::new(&answers(3), "2210345683", Some(1)));
    // 4 and 5: two sheets claiming student 4.
    sheets.push(Marks::new(&answers(4), &student(4), Some(0)));
    sheets.push(Marks::new(&answers(5), &student(4), Some(1)));
    for (k, m) in sheets.iter().enumerate() {
        write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, m, 50 + k as u64)),
        );
    }
    let def = definition("ID mistakes", &roster);
    let mut p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let idq: Vec<IdItem> = p
        .queue(Some(Lane::Identity))
        .iter()
        .map(|i| {
            (
                i.id,
                i.booklet.unwrap(),
                i.code.clone(),
                i.sub_reason.clone(),
                i.suggestion.clone(),
            )
        })
        .collect();
    let bk: Vec<usize> = (0..6).map(|k| booklet_of(&p, k)).collect();
    let item_of = |k: u16| {
        let bi = bk[usize::from(k)];
        idq.iter()
            .find(|i| i.1 == bi)
            .cloned()
            .unwrap_or_else(|| panic!("sheet {k}: no identity item"))
    };
    // Control: settled, no item.
    let b0 = bk[0];
    assert!(idq.iter().all(|i| i.1 != b0));
    assert_eq!(p.session.booklets[b0].student(), Some(student(0).as_str()));
    let sugg = |s: &str| Some(Decision::Student(s.to_string()));
    // Blank column: id_blank, the one roster ID that fits is proposed.
    let (_, _, code, sub, s1) = item_of(1);
    assert_eq!(
        (code.as_str(), sub.as_deref()),
        ("IDN-004", Some("id_blank"))
    );
    assert_eq!(s1, sugg(&student(1)));
    // Two bubbles in a column: id_ambiguous, the roster resolves the proposal.
    let (_, _, code, sub, s2) = item_of(2);
    assert_eq!(
        (code.as_str(), sub.as_deref()),
        ("IDN-004", Some("id_ambiguous"))
    );
    assert_eq!(s2, sugg(&student(2)));
    // Not on the roster: the writer-error neighbour is proposed.
    let (_, _, code, sub, s3) = item_of(3);
    assert_eq!(
        (code.as_str(), sub.as_deref()),
        ("IDN-004", Some("not_on_roster"))
    );
    assert_eq!(s3, sugg(&student(3)));
    // One ID on two sheets: IDN-005 on both.
    let (i4, _, c4, _, _) = item_of(4);
    let (i5, _, c5, _, _) = item_of(5);
    assert_eq!((c4.as_str(), c5.as_str()), ("IDN-005", "IDN-005"));
    // The suggestions are usable: accepting them settles every identity.
    for k in [1u16, 2, 3] {
        let (id, ..) = item_of(k);
        p.apply(
            id,
            Action::Resolve {
                decision: Decision::Acknowledge,
                accepted_suggestion: true,
                note: String::new(),
            },
        )
        .unwrap();
        let bi = bk[usize::from(k)];
        assert_eq!(p.session.booklets[bi].student(), Some(student(k).as_str()));
    }
    // The duplicate: one sheet is student 4's, the other student 5's (the reviewer's call,
    // with its reason: an override of the engine's proposal).
    p.apply(
        i4,
        Action::Resolve {
            decision: Decision::Student(student(4)),
            accepted_suggestion: false,
            note: String::new(),
        },
    )
    .unwrap();
    p.apply_with_reason(
        i5,
        Action::Resolve {
            decision: Decision::Student(student(5)),
            accepted_suggestion: false,
            note: String::new(),
        },
        Some(omr_app::review::ReviewReason::code("DUPLICATE_RESOLVED")),
    )
    .unwrap();
    assert!(p.queue(Some(Lane::Identity)).is_empty());
    let _ = ItemKind::Identity;
}

/// CR-17: a number on no roster entry whose nearest roster student already holds another
/// sheet gets no one-key proposal of that student (accepting it would be refused with IDN-005,
/// a dead end); the student stays a listed candidate.
#[test]
fn c10_a_roster_neighbour_who_already_holds_a_sheet_is_never_proposed() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    let roster: Vec<String> = (0..3).map(student).collect();
    // 0: student 0's own sheet; 1: a number one digit off student 0's, on no roster entry.
    let sheets = [
        Marks::new(&answers(0), &student(0), Some(0)),
        Marks::new(&answers(1), "2210345680", Some(1)),
    ];
    for (k, m) in sheets.iter().enumerate() {
        write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, m, 80 + k as u64)),
        );
    }
    let def = definition("ID neighbour", &roster);
    let p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let (b0, b1) = (booklet_of(&p, 0), booklet_of(&p, 1));
    assert_eq!(p.session.booklets[b0].student(), Some(student(0).as_str()));
    let q = p.queue(Some(Lane::Identity));
    let item = q
        .iter()
        .find(|i| i.booklet == Some(b1))
        .expect("an identity item for the stray number");
    assert_eq!(
        (item.code.as_str(), item.sub_reason.as_deref()),
        ("IDN-004", Some("not_on_roster"))
    );
    assert_ne!(
        item.suggestion,
        Some(Decision::Student(student(0))),
        "student 0 already holds sheet 0"
    );
    assert!(
        p.session.booklets[b1]
            .identity
            .candidates
            .iter()
            .any(|c| c.0 == student(0)),
        "still listed as a candidate"
    );
}
