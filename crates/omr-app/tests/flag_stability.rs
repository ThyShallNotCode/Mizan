//! Flag stability in the exports (spec §12.2 "Flag stability", F2): a graded exam's item
//! analysis carries, for every rank 1–4 flag, the share of 1,000 bootstrap resamples in which it
//! fires again, and a flag below 50 % or failing at one end of its CI is labelled low
//! confidence. A genuine mis-key stays a confident SCO-001; one top scorer choosing a
//! distractor on an item nearly everyone answers correctly is low confidence. The bootstrap is
//! seeded from the data's content, so the exports are byte-identical at 1 and 4 threads and
//! through two key histories. A confident probable mis-key is a Scoring-lane SCO-001 ERROR
//! item that blocks release until the instructor decides it (C1); the one-outlier flag never
//! blocks. Synthetic IAU sheets and a synthetic key only.

mod common;

use common::*;
use omr_app::review::{Action, Decision, ItemState, Lane, ReviewReason};
use omr_app::{ExamProject, FormKey, KeySource};
use std::path::{Path, PathBuf};

const N: u16 = 10;
const STUDENTS: u16 = 40;
/// The key of question `q` (0-based): A on the mis-keyed Q1 (the class's answer is B).
fn keyed(q: u16) -> u8 {
    match q {
        0 => 0,
        1 => 3,
        _ => (q % 5) as u8,
    }
}

/// What student `k` (ability rank: higher is stronger) marks.
fn answer(k: u16, q: u16) -> u8 {
    match q {
        // Mis-keyed: the stronger half answers B (the true answer), the weaker half A (the key).
        0 => u8::from(k >= STUDENTS / 2),
        // Nearly everyone answers D (the key); the top scorer alone chose C.
        1 => {
            if k == STUDENTS - 1 {
                2
            } else {
                3
            }
        }
        // Ability items: correct when k clears the item's difficulty, else a fixed distractor.
        _ => {
            let difficulty = 4 * (q - 1) + (k * 7 + q * 3) % 5;
            if k >= difficulty {
                keyed(q)
            } else {
                (keyed(q) + 1 + (k + q) as u8 % 4) % 5
            }
        }
    }
}

fn scans(dir: &Path) -> Vec<PathBuf> {
    let e = iau();
    (0..STUDENTS)
        .map(|k| {
            let answers: Vec<Option<u8>> = (0..N).map(|q| Some(answer(k, q))).collect();
            let img = sheet(
                &e,
                &Marks::new(&answers, &format!("22104520{k:02}"), Some(0)),
                500 + u64::from(k),
            );
            write(dir, &format!("s{k:02}.png"), &png(&img))
        })
        .collect()
}

fn flags_of(csv: &str, label: &str) -> String {
    let mut lines = csv.lines();
    let header: Vec<&str> = lines
        .find(|l| l.starts_with("Item,") || l.contains(",Flags"))
        .expect("header")
        .split(',')
        .collect();
    let col = header.iter().position(|h| *h == "Flags").expect("Flags");
    lines
        .map(|l| l.split(',').collect::<Vec<_>>())
        .find(|c| c.first() == Some(&label))
        .and_then(|c| c.get(col).map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn exported_flags_carry_their_stability_and_low_confidence_label() {
    let tmp = tempfile::tempdir().unwrap();
    let inputs = scans(&tmp.path().join("scans"));
    let key = KeySource::Provided {
        forms: vec![FormKey {
            form: "A".into(),
            answers: (0..N)
                .map(|q| char::from(b'A' + keyed(q)).to_string())
                .collect(),
        }],
    };
    let def = legacy_def("Final", N, &["A"], key);
    let mut csvs = Vec::new();
    for threads in [1, 4] {
        let out = tmp.path().join(format!("t{threads}"));
        let p = grade(&def, &inputs, &out, threads);
        // The synchronous path stays bootstrap-free until the job has run for this score run.
        let st = p.flag_stability().expect("stability job");
        assert_eq!(st.resamples, 1000);
        assert_eq!(st.n, usize::from(STUDENTS));
        let a = p.analytics().unwrap();
        assert!(a.stability.is_some());
        let miskey = st
            .flags
            .iter()
            .find(|f| f.label.ends_with("Q1") && f.rule == omr_analytics::Rule::ProbableMiskey)
            .expect("rule 1 on the mis-keyed item");
        assert!(miskey.share.unwrap() > 0.95, "{miskey:?}");
        assert!(!miskey.low_confidence, "{miskey:?}");
        let outlier = st
            .flags
            .iter()
            .find(|f| f.label.ends_with("Q2") && f.rule == omr_analytics::Rule::ProbableMiskey)
            .expect("rule 1 on the one-outlier item");
        assert!(outlier.low_confidence, "{outlier:?}");
        let csv = std::fs::read_to_string(out.join("item_analysis.csv")).unwrap();
        let q1 = flags_of(&csv, "Q1");
        let q2 = flags_of(&csv, "Q2");
        assert!(
            q1.contains("SCO-001 Probable mis-key: B") && q1.contains("bootstrap resamples"),
            "{q1}"
        );
        assert!(!q1.contains("proposed"), "{q1}");
        // A low-confidence rank-1 flag is the non-blocking SCO-002, marked (proposed).
        assert!(
            q2.contains("SCO-002 Probable mis-key: C (proposed)"),
            "{q2}"
        );
        let html = std::fs::read_to_string(out.join("report.html")).unwrap();
        assert!(html.contains("bootstrap resamples"));
        csvs.push(csv);
        p.close().unwrap();
    }
    assert_eq!(
        csvs[0], csvs[1],
        "item analysis identical at 1 and 4 threads"
    );
}

fn key_of(answers: impl Fn(u16) -> u8) -> KeySource {
    KeySource::Provided {
        forms: vec![FormKey {
            form: "A".into(),
            answers: (0..N)
                .map(|q| char::from(b'A' + answers(q)).to_string())
                .collect(),
        }],
    }
}

fn miskey_items(p: &ExamProject) -> Vec<&omr_app::ReviewItem> {
    p.session
        .review
        .items
        .iter()
        .filter(|i| i.code == "SCO-001")
        .collect()
}

#[test]
fn a_planted_mis_key_blocks_release_until_the_instructor_decides_it() {
    // C1: SCO-001 is ERROR, Exam scope, "acknowledge" (§12.2, Appendix A); an open ERROR
    // blocks release (§8.1). The confident mis-key (Q1) is queued in the Scoring lane and
    // blocks; the one-outlier flag (Q2, low confidence) is not queued and never blocks.
    let tmp = tempfile::tempdir().unwrap();
    let inputs = scans(&tmp.path().join("scans"));
    let def = legacy_def("Final", N, &["A"], key_of(keyed));
    let out = tmp.path().join("out");
    let mut p = grade(&def, &inputs, &out, 2);
    let items = miskey_items(&p);
    assert_eq!(items.len(), 1, "{items:#?}");
    let it = items[0].clone();
    assert_eq!(it.lane, Lane::Scoring);
    assert_eq!(it.severity, omr_model::Severity::Error);
    assert_eq!(it.sub_reason.as_deref(), Some("Q1:B"));
    assert_eq!(it.question, Some(0));
    assert!(it.is_open());
    assert!(it.suggestion.is_none(), "flags never re-key on their own");
    assert!(it.detail.contains("bootstrap resamples"), "{}", it.detail);
    assert!(p.queue(Some(Lane::Scoring)).iter().any(|i| i.id == it.id));
    // Nothing else blocks these clean synthetic sheets: only the mis-key does.
    let why = p.session.release_blocked().expect("blocked by the mis-key");
    assert!(why.contains("probable mis-key (SCO-001)"), "{why}");
    let err = p.release(None).unwrap_err().to_string();
    assert!(err.contains("SCO-001"), "{err}");
    // The exports say so too.
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["open_items_by_code"]["SCO-001"], 1);
    assert_eq!(summary["miskey_check"], "done");
    assert_eq!(summary["item_analysis"]["flags_by_code"]["SCO-001"], 1);
    assert!(
        summary["item_analysis"]["flags_by_code"]["SCO-002 proposed_miskey"]
            .as_u64()
            .unwrap()
            >= 1
    );
    // Keeping the key needs a reason code and a note (§12.2 "keep with a note").
    let keep = || Action::Resolve {
        decision: Decision::Acknowledge,
        accepted_suggestion: false,
        note: String::new(),
    };
    assert!(p.apply(it.id, keep()).is_err());
    assert!(
        p.apply_with_reason(it.id, keep(), Some(ReviewReason::code("ADJUDICATION")))
            .is_err()
    );
    // Nothing a mark item takes decides a mis-key.
    let blank = Action::Resolve {
        decision: Decision::Blank,
        accepted_suggestion: false,
        note: String::new(),
    };
    assert!(p.apply(it.id, blank).is_err());
    p.apply_with_reason(
        it.id,
        keep(),
        Some(ReviewReason::with_note(
            "ADJUDICATION",
            "checked with the course coordinator: A is right",
        )),
    )
    .unwrap();
    assert_eq!(
        p.session.review.item(it.id).unwrap().state,
        ItemState::Resolved
    );
    assert_eq!(p.session.release_blocked(), None);
    // A later key change elsewhere makes a new score run: the check reads "computing…" until
    // it has run, and the kept decision stays.
    p.set_key(0, 5, "*").unwrap();
    assert!(
        p.session.release_blocked().unwrap().contains("computing"),
        "{:?}",
        p.session.release_blocked()
    );
    p.release(None).expect("judged, then released");
    assert_eq!(miskey_items(&p).len(), 1);
    assert_eq!(
        p.session.review.item(it.id).unwrap().state,
        ItemState::Resolved
    );
    p.close().unwrap();
}

#[test]
fn a_mis_key_is_decided_by_changing_the_key_and_the_flag_is_gone() {
    // C1: "change key to d" is an audited key version (KEY_ERROR); the rescored exam no
    // longer flags the item, and the release is allowed.
    let tmp = tempfile::tempdir().unwrap();
    let inputs = scans(&tmp.path().join("scans"));
    let def = legacy_def("Final", N, &["A"], key_of(keyed));
    let out = tmp.path().join("out");
    let mut p = grade(&def, &inputs, &out, 2);
    let id = miskey_items(&p)[0].id;
    let v0 = p.session.key.as_ref().unwrap().version;
    p.apply(
        id,
        Action::Resolve {
            decision: Decision::Option(1),
            accepted_suggestion: false,
            note: String::new(),
        },
    )
    .unwrap();
    let key = p.session.key.as_ref().unwrap();
    assert_eq!(key.version, v0 + 1);
    assert_eq!(key.forms[0][0].text(), "B");
    p.export(&out).unwrap();
    assert_eq!(miskey_items(&p).len(), 1, "decided, not re-queued");
    assert!(!miskey_items(&p)[0].is_open());
    let csv = std::fs::read_to_string(out.join("item_analysis.csv")).unwrap();
    assert!(
        !flags_of(&csv, "Q1").contains("SCO-001"),
        "{}",
        flags_of(&csv, "Q1")
    );
    p.release(None).expect("nothing blocks");
    p.close().unwrap();
}

#[test]
fn accepting_a_second_answer_or_voiding_decides_a_mis_key() {
    // C1: "accept key ∪ {d}" and "void" (§12.2), each an audited key version.
    for (decision, text) in [(Decision::Multi(Vec::new()), "A|B"), (Decision::Void, "*")] {
        let tmp = tempfile::tempdir().unwrap();
        let inputs = scans(&tmp.path().join("scans"));
        let def = legacy_def("Final", N, &["A"], key_of(keyed));
        let mut p =
            ExamProject::grade(&def, &inputs, &options(&tmp.path().join("o"), 2), &|_| {}).unwrap();
        let id = miskey_items(&p)[0].id;
        p.apply(
            id,
            Action::Resolve {
                decision,
                accepted_suggestion: false,
                note: String::new(),
            },
        )
        .unwrap();
        assert_eq!(p.session.key.as_ref().unwrap().forms[0][0].text(), text);
        p.judge_flags().unwrap();
        assert!(miskey_items(&p).iter().all(|i| !i.is_open()));
        assert_eq!(p.session.release_blocked(), None, "{text}");
    }
}

#[test]
fn the_same_content_through_two_key_histories_gives_identical_flags() {
    // C8: the bootstrap is seeded from the analysis content, never the key's version history:
    // grading with the key, and grading with another key then correcting it to the same key
    // (key version 2), give the same flag shares and labels.
    let tmp = tempfile::tempdir().unwrap();
    let inputs = scans(&tmp.path().join("scans"));
    let def = legacy_def("Final", N, &["A"], key_of(keyed));
    let a = grade(&def, &inputs, &tmp.path().join("a"), 2);
    let other = legacy_def(
        "Final",
        N,
        &["A"],
        key_of(|q| if q == 4 { (keyed(q) + 1) % 5 } else { keyed(q) }),
    );
    let mut b = grade(&other, &inputs, &tmp.path().join("b"), 2);
    let right = char::from(b'A' + keyed(4)).to_string();
    b.set_key(0, 5, &right).unwrap();
    b.export(&tmp.path().join("b")).unwrap();
    assert_ne!(
        a.session.key.as_ref().unwrap().version,
        b.session.key.as_ref().unwrap().version
    );
    assert_ne!(a.current_run_id(), b.current_run_id(), "the run IDs differ");
    let sa = a.flag_stability().unwrap();
    let sb = b.flag_stability().unwrap();
    assert_eq!(sa.seed, sb.seed);
    assert_eq!(sa.flags, sb.flags);
    assert_eq!(sa.item_ci, sb.item_ci);
    let ia = std::fs::read(tmp.path().join("a").join("item_analysis.csv")).unwrap();
    let ib = std::fs::read(tmp.path().join("b").join("item_analysis.csv")).unwrap();
    assert_eq!(ia, ib, "item analysis identical through two key histories");
}
