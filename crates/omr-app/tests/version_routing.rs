//! Version routing on IAU sheets (§10.3 version backstops), synthetic data only: which key
//! scores a sheet, for every version the exam may declare crossed with what a sheet's version
//! bubble may show. A confident letter the exam does not declare is held (SCO-005
//! `undeclared_version`) and never scored, on a one-version exam too; a definition saved before
//! the backstops keeps its earlier routing byte for byte.
//!
//! The keys overlap on half the questions and the students answer their own version's key
//! well, so a sheet scored under another version's key would score far above chance: holding
//! it never relies on SCO-005 `near_chance`.

mod common;

use common::{Marks, bubble, grade, iau, legacy_def, png, sheet, source_of, write};
use omr_app::model::FormSource;
use omr_app::{Action, Decision, ExamDefinition, ExamProject, FormKey, ItemKind, KeySource, Lane};
use omr_forms::legacy::LegacyField;
use std::path::{Path, PathBuf};

const N: u16 = 20;

/// The key of version `v` (0 = A … 3 = D): even questions are the same on every version,
/// odd ones differ.
fn key(v: u8, q: u16) -> u8 {
    let base = ((q * 3) % 5) as u8;
    if q.is_multiple_of(2) {
        base
    } else {
        (base + v) % 5
    }
}

/// A strong student of version `v`: the key, but wrong on every seventh question.
fn answers(v: u8) -> Vec<Option<u8>> {
    (0..N)
        .map(|q| {
            let k = key(v, q);
            Some(if q % 7 == 3 { (k + 1) % 5 } else { k })
        })
        .collect()
}

fn hand_score(answered_as: u8, scored_as: u8) -> usize {
    (0..N)
        .filter(|&q| answers(answered_as)[usize::from(q)] == Some(key(scored_as, q)))
        .count()
}

/// What one sheet's version bubble shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shows {
    /// A confident letter (0 = A).
    Letter(u8),
    /// Nothing bubbled.
    Blank,
    /// Two letters bubbled (A and C): not read as a version.
    Unclear,
}

/// The stack: one sheet per observation, `(file stem, what the bubble shows, the version the
/// student answered)`. `c-answers-a` bubbles C but answers version A's key.
const SHEETS: [(&str, Shows, u8); 7] = [
    ("a", Shows::Letter(0), 0),
    ("b", Shows::Letter(1), 1),
    ("c", Shows::Letter(2), 2),
    ("d", Shows::Letter(3), 3),
    ("blank", Shows::Blank, 0),
    ("unclear", Shows::Unclear, 0),
    ("c-answers-a", Shows::Letter(2), 0),
];

fn student(k: usize) -> String {
    format!("2210345{k:03}")
}

fn write_stack(dir: &Path) -> PathBuf {
    let e = iau();
    for (k, (stem, shows, v)) in SHEETS.iter().enumerate() {
        let mut m = Marks::new(&answers(*v), &student(k), None);
        match shows {
            Shows::Letter(l) => m.version = Some(*l),
            Shows::Blank => {}
            Shows::Unclear => {
                m.extra.push(bubble(&e, LegacyField::Version, 0, 0));
                m.extra.push(bubble(&e, LegacyField::Version, 0, 2));
            }
        }
        write(
            dir,
            &format!("{k}-{stem}.png"),
            &png(&sheet(&e, &m, 300 + k as u64)),
        );
    }
    dir.to_path_buf()
}

/// An IAU exam declaring `forms`, each with its own key; `hold` as a definition built today.
fn exam(forms: &[&str], hold: bool) -> ExamDefinition {
    let keys = forms
        .iter()
        .map(|f| FormKey {
            form: (*f).into(),
            answers: (0..N)
                .map(|q| char::from(b'A' + key(f.as_bytes()[0] - b'A', q)).to_string())
                .collect(),
        })
        .collect();
    let mut d = legacy_def("Routing", N, forms, KeySource::Provided { forms: keys });
    d.hold_undeclared_versions = hold;
    d
}

/// The observation index of booklet `bi` (from its file name `k-stem.png`).
fn sheet_no(p: &ExamProject, bi: usize) -> usize {
    source_of(p, bi).split('-').next().unwrap().parse().unwrap()
}

fn letter(v: u8) -> String {
    char::from(b'A' + v).to_string()
}

#[test]
fn every_version_set_routes_every_bubble_and_holds_undeclared_letters() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    for active in [
        vec!["A"],
        vec!["C"],
        vec!["A", "B"],
        vec!["A", "B", "C", "D"],
    ] {
        let name = active.join("");
        let def = exam(&active, true);
        let out = tmp.path().join(&name);
        let p = grade(&def, std::slice::from_ref(&scans), &out, 2);
        let s = &p.session;
        assert_eq!(s.booklets.len(), SHEETS.len(), "[{name}]");
        let sc = p.scoring.as_ref().expect("scored");
        let run = sc.run.as_ref().unwrap();
        let results = std::fs::read_to_string(out.join("results.csv")).unwrap();
        for (bi, b) in s.booklets.iter().enumerate() {
            let k = sheet_no(&p, bi);
            let (stem, shows, answered) = SHEETS[k];
            let ctx = format!("[{name}] sheet {stem}");
            let sco005: Vec<&str> = b
                .exceptions
                .iter()
                .filter(|e| e.code == "SCO-005")
                .map(|e| e.sub_reason.as_deref().unwrap_or(""))
                .collect();
            let declared = match shows {
                Shows::Letter(l) => active.iter().position(|f| *f == letter(l)),
                _ => None,
            };
            match (shows, declared) {
                // A declared letter: its own key.
                (Shows::Letter(_), Some(f)) => {
                    assert_eq!(b.form, Some(f), "{ctx}");
                    assert_eq!(b.form_source, FormSource::Bubbled, "{ctx}");
                    assert!(sco005.is_empty(), "{ctx}: {sco005:?}");
                }
                // A letter the exam does not declare: held, never scored, never given a
                // provisional version.
                (Shows::Letter(l), None) => {
                    assert_eq!(b.form, None, "{ctx}");
                    assert_eq!(b.form_source, FormSource::Unknown, "{ctx}");
                    assert_eq!(sco005, ["undeclared_version"], "{ctx}");
                    let e = b.exceptions.iter().find(|e| e.code == "SCO-005").unwrap();
                    assert!(
                        e.detail.contains(&format!("bubbles version {}", letter(l)))
                            && e.detail.contains(&active.join(", ")),
                        "{ctx}: {}",
                        e.detail
                    );
                    assert_eq!(sc.row_of[bi], None, "{ctx}: never in the score run");
                    assert!(!results.contains(&student(k)), "{ctx}: not in results.csv");
                    let items: Vec<_> = p
                        .queue(Some(Lane::Scoring))
                        .into_iter()
                        .filter(|i| i.booklet == Some(bi))
                        .collect();
                    assert_eq!(items.len(), 1, "{ctx}");
                    assert_eq!(items[0].kind, ItemKind::Form, "{ctx}");
                    assert_eq!(
                        items[0].sub_reason.as_deref(),
                        Some("undeclared_version"),
                        "{ctx}"
                    );
                    // The answers suggest a version only when they single one out by 10 nats:
                    // the sheet that bubbles C but answers A's key on an A,B exam.
                    let want = (stem == "c-answers-a" && active.len() > 1).then_some(0u8);
                    assert_eq!(items[0].suggestion, want.map(Decision::Form), "{ctx}");
                }
                // No version read: the one version of a one-version exam ...
                (_, _) if active.len() == 1 => {
                    assert_eq!(b.form, Some(0), "{ctx}");
                    assert_eq!(b.form_source, FormSource::Shared, "{ctx}");
                    assert!(sco005.is_empty(), "{ctx}: {sco005:?}");
                }
                // ... else inferred from the answers (provisional, reviewed).
                (_, _) => {
                    assert_eq!(b.form, Some(0), "{ctx}: answers A's key");
                    assert_eq!(b.form_source, FormSource::Inferred, "{ctx}");
                    assert_eq!(sco005, ["inferred"], "{ctx}");
                }
            }
            if shows == Shows::Unclear {
                assert_eq!(b.declared_text, "?", "{ctx}");
            }
            // Every scored sheet scores its hand count under the key of its version.
            if let Some(f) = b.form {
                assert!(results.contains(&student(k)), "{ctx}: in results.csv");
                let row = sc.row_of[bi].expect("a sheet with a version is scored");
                let scored_as = active[f].as_bytes()[0] - b'A';
                assert_eq!(
                    run.students[row].raw.to_string(),
                    hand_score(answered, scored_as).to_string(),
                    "{ctx}"
                );
            }
        }
        assert!(s.release_blocked().is_some(), "[{name}]");
        p.close().unwrap();
    }
}

#[test]
fn a_reviewer_gives_a_held_sheet_its_version_and_it_is_scored() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    let mut p = grade(
        &exam(&["A", "B"], true),
        std::slice::from_ref(&scans),
        &tmp.path().join("out"),
        2,
    );
    let held: Vec<(u64, usize)> = p
        .queue(Some(Lane::Scoring))
        .iter()
        .filter(|i| i.sub_reason.as_deref() == Some("undeclared_version"))
        .map(|i| (i.id, i.booklet.unwrap()))
        .collect();
    assert_eq!(held.len(), 3, "c, d and c-answers-a");
    for (id, bi) in held {
        // The student meant version A (only `c-answers-a` did; the others are graded under A
        // all the same when the reviewer says so).
        p.apply(
            id,
            Action::Resolve {
                decision: Decision::Form(0),
                accepted_suggestion: false,
                note: String::new(),
            },
        )
        .unwrap();
        let b = &p.session.booklets[bi];
        assert_eq!(b.form, Some(0));
        assert_eq!(b.form_source, FormSource::Reviewed);
        let sc = p.scoring.as_ref().unwrap();
        let row = sc.row_of[bi].expect("scored after the decision");
        let (_, _, answered) = SHEETS[sheet_no(&p, bi)];
        assert_eq!(
            sc.run.as_ref().unwrap().students[row].raw.to_string(),
            hand_score(answered, 0).to_string()
        );
    }
}

#[test]
fn a_definition_saved_before_the_backstop_keeps_its_routing_and_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    // The definition of an existing project: the flag is not in its JSON and reads as off,
    // and an off flag is never written, so the saved definition keeps its bytes.
    let old = exam(&["A"], false);
    let json = serde_json::to_string(&old).unwrap();
    assert!(!json.contains("hold_undeclared_versions"), "{json}");
    let back: ExamDefinition = serde_json::from_str(&json).unwrap();
    assert!(!back.hold_undeclared_versions);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    let new = exam(&["A"], true);
    let back: ExamDefinition = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
    assert!(back.hold_undeclared_versions);
    // One version: every sheet takes it, whatever its bubble shows (the earlier shortcut).
    let p = grade(
        &old,
        std::slice::from_ref(&scans),
        &tmp.path().join("one"),
        2,
    );
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let (stem, _, _) = SHEETS[sheet_no(&p, bi)];
        assert_eq!(b.form, Some(0), "{stem}");
        assert_eq!(b.form_source, FormSource::Bubbled, "{stem}");
        assert!(b.exceptions.iter().all(|e| e.code != "SCO-005"), "{stem}");
        assert!(p.scoring.as_ref().unwrap().row_of[bi].is_some(), "{stem}");
    }
    p.close().unwrap();
    // Two versions: an undeclared letter is inferred as before (provisional, SCO-005).
    let p = grade(
        &exam(&["A", "B"], false),
        std::slice::from_ref(&scans),
        &tmp.path().join("two"),
        2,
    );
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let (stem, shows, _) = SHEETS[sheet_no(&p, bi)];
        if matches!(shows, Shows::Letter(2) | Shows::Letter(3)) {
            assert_eq!(b.form_source, FormSource::Inferred, "{stem}");
            assert!(b.form.is_some(), "{stem}");
            assert!(
                b.exceptions
                    .iter()
                    .any(|e| e.sub_reason.as_deref() == Some("inferred")),
                "{stem}"
            );
        }
    }
    p.close().unwrap();
}

#[test]
fn a_new_exam_from_the_new_exam_form_holds_undeclared_versions() {
    let i =
        omr_app::define::Inputs::new(omr_app::define::Sheet::Template("iau-ccsit-v1".into()), N);
    let (d, _) = omr_app::define::define(&i).unwrap();
    assert!(d.hold_undeclared_versions);
}

/// An exam whose versions all carry version A's key, points and mapping (identical versions).
fn identical_exam(forms: &[&str], hold: bool) -> ExamDefinition {
    let keys = forms
        .iter()
        .map(|f| FormKey {
            form: (*f).into(),
            answers: (0..N).map(|q| letter(key(0, q))).collect(),
        })
        .collect();
    let mut d = legacy_def("Identical", N, forms, KeySource::Provided { forms: keys });
    d.hold_undeclared_versions = hold;
    d
}

/// The stack's files of the observations named.
fn only(scans: &Path, stems: &[&str]) -> Vec<PathBuf> {
    SHEETS
        .iter()
        .enumerate()
        .filter(|(_, (stem, _, _))| stems.contains(stem))
        .map(|(k, (stem, _, _))| scans.join(format!("{k}-{stem}.png")))
        .collect()
}

/// The Form column of results.csv for the student of observation `k`.
fn form_cell(out: &Path, k: usize) -> String {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(out.join("results.csv"))
        .unwrap();
    let form = r
        .headers()
        .unwrap()
        .iter()
        .position(|h| h == "Form")
        .unwrap();
    r.records()
        .map(|x| x.unwrap())
        .find(|x| x.iter().any(|c| c.contains(&student(k))))
        .map(|x| x[form].to_string())
        .unwrap_or_else(|| panic!("student of sheet {k} not in results.csv"))
}

#[test]
fn identical_versions_score_an_undeclared_sheet_under_the_shared_key_without_review() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    let inputs = only(&scans, &["a", "b", "blank", "unclear"]);
    let out = tmp.path().join("out");
    let p = grade(&identical_exam(&["A", "B"], true), &inputs, &out, 2);
    let sc = p.scoring.as_ref().unwrap();
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let k = sheet_no(&p, bi);
        let (stem, shows, answered) = SHEETS[k];
        let sco005: Vec<_> = b
            .exceptions
            .iter()
            .filter(|e| e.code == "SCO-005")
            .collect();
        if let Shows::Letter(l) = shows {
            assert_eq!(b.form, Some(usize::from(l)), "{stem}");
            assert_eq!(b.form_source, FormSource::Bubbled, "{stem}");
            assert!(sco005.is_empty(), "{stem}");
            assert_eq!(form_cell(&out, k), letter(l), "{stem}");
        } else {
            // Scored under the key both versions share, counted, never queued.
            assert_eq!(b.form, Some(0), "{stem}");
            assert_eq!(b.form_source, FormSource::Shared, "{stem}");
            assert_eq!(sco005.len(), 1, "{stem}");
            assert_eq!(sco005[0].sub_reason.as_deref(), Some("identical_forms"));
            assert_eq!(sco005[0].severity, omr_model::Severity::Info, "{stem}");
            // Per-version columns: "version not declared", never version A.
            assert_eq!(form_cell(&out, k), "", "{stem}");
        }
        let row = sc.row_of[bi].expect("scored");
        assert_eq!(
            sc.run.as_ref().unwrap().students[row].raw.to_string(),
            hand_score(answered, 0).to_string(),
            "{stem}"
        );
    }
    assert!(p.queue(None).iter().all(|i| i.code != "SCO-005"));
    let r = p.results_vm().unwrap();
    assert!(
        r.readiness
            .contains(&(true, "0 forms inferred".to_string())),
        "{:?}",
        r.readiness
    );
    p.close().unwrap();
    // A definition saved before the backstops keeps queueing them (CR-16), byte for byte.
    let p = grade(
        &identical_exam(&["A", "B"], false),
        &inputs,
        &tmp.path().join("old"),
        2,
    );
    let inferred = p
        .queue(Some(Lane::Scoring))
        .iter()
        .filter(|i| i.sub_reason.as_deref() == Some("inferred"))
        .count();
    assert_eq!(inferred, 2);
    p.close().unwrap();
}

/// The open Scoring-lane form items of booklets whose sheets are `stems`.
fn form_items(p: &ExamProject, stems: &[&str]) -> Vec<u64> {
    p.queue(Some(Lane::Scoring))
        .iter()
        .filter(|i| i.kind == ItemKind::Form)
        .filter(|i| stems.contains(&SHEETS[sheet_no(p, i.booklet.unwrap())].0))
        .map(|i| i.id)
        .collect()
}

fn apply_to_group(p: &mut ExamProject, id: u64) -> Result<usize, String> {
    p.apply(
        id,
        Action::ApplyToGroup {
            decision: Decision::Form(0),
        },
    )
    .map(|o| o.affected.len())
    .map_err(|e| e.to_string())
}

#[test]
fn versions_are_confirmed_as_a_group_only_by_a_10_nat_margin() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    let inputs = only(&scans, &["a", "b", "blank", "unclear"]);
    // Identical keys in a definition saved before the backstops: the answers cannot tell the
    // versions apart (margin 0), so the two inferred versions are one group that is refused.
    let mut p = grade(
        &identical_exam(&["A", "B"], false),
        &inputs,
        &tmp.path().join("tied"),
        2,
    );
    let ids = form_items(&p, &["blank", "unclear"]);
    assert_eq!(ids.len(), 2);
    let e = apply_to_group(&mut p, ids[0]).unwrap_err();
    assert!(e.contains("10 nats") && e.contains("on its own"), "{e}");
    assert_eq!(
        form_items(&p, &["blank", "unclear"]).len(),
        2,
        "nothing changed"
    );
    p.close().unwrap();
    // Versions whose keys differ: the answers single out version A by far more than 10 nats.
    let mut p = grade(
        &exam(&["A", "B"], false),
        &inputs,
        &tmp.path().join("clear"),
        2,
    );
    let ids = form_items(&p, &["blank", "unclear"]);
    assert_eq!(ids.len(), 2);
    assert_eq!(apply_to_group(&mut p, ids[0]), Ok(2));
    assert!(form_items(&p, &["blank", "unclear"]).is_empty());
    p.close().unwrap();
    // A sheet bubbling a version the exam does not declare is decided on its own, whatever
    // its margin: C and D (no suggestion), and C answering A's key (suggested A).
    let mut p = grade(
        &exam(&["A", "B"], true),
        std::slice::from_ref(&scans),
        &tmp.path().join("held"),
        2,
    );
    let held = form_items(&p, &["c", "d", "c-answers-a"]);
    assert_eq!(held.len(), 3);
    for id in held {
        let e = apply_to_group(&mut p, id).unwrap_err();
        assert!(e.contains("does not declare"), "{e}");
    }
    assert_eq!(form_items(&p, &["c", "d", "c-answers-a"]).len(), 3);
    p.close().unwrap();
}

/// The keys of a definition with a provided key.
fn keys_of(d: &ExamDefinition) -> Vec<FormKey> {
    let KeySource::Provided { forms } = &d.key else {
        panic!("a provided key")
    };
    forms.clone()
}

/// The suggestion of the open Form item of the sheet `stem`, and its booklet.
fn form_suggestion(p: &ExamProject, stem: &str) -> (Option<Decision>, usize) {
    let item = p
        .queue(Some(Lane::Scoring))
        .into_iter()
        .find(|i| i.kind == ItemKind::Form && SHEETS[sheet_no(p, i.booklet.unwrap())].0 == stem)
        .unwrap_or_else(|| panic!("no Form item for {stem}"));
    (item.suggestion.clone(), item.booklet.unwrap())
}

#[test]
fn a_key_change_refreshes_a_held_sheets_suggestion_only_by_a_10_nat_margin() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    let def = exam(&["A", "B"], true);
    let mut p = grade(
        &def,
        std::slice::from_ref(&scans),
        &tmp.path().join("out"),
        2,
    );
    // `c-answers-a` bubbles C and answers A's key: suggested A.
    let (s, bi) = form_suggestion(&p, "c-answers-a");
    assert_eq!(s, Some(Decision::Form(0)));
    let graded = format!("{:?}", p.session.booklets[bi].inference);
    // A key giving both versions the same answers: the answers single out no version, and the
    // held sheets' suggestions follow the key.
    p.replace_key(keys_of(&identical_exam(&["A", "B"], true)), None, None)
        .unwrap();
    assert_ne!(format!("{:?}", p.session.booklets[bi].inference), graded);
    for stem in ["c", "d", "c-answers-a"] {
        assert_eq!(form_suggestion(&p, stem).0, None, "{stem}");
    }
    // The versions' own keys again: A suggested again, C's and D's answers single out neither
    // A nor B by 10 nats.
    p.replace_key(keys_of(&def), None, None).unwrap();
    assert_eq!(
        form_suggestion(&p, "c-answers-a").0,
        Some(Decision::Form(0))
    );
    assert_eq!(format!("{:?}", p.session.booklets[bi].inference), graded);
    for stem in ["c", "d"] {
        assert_eq!(form_suggestion(&p, stem).0, None, "{stem}");
    }
    p.close().unwrap();
}

#[test]
fn a_sheet_without_a_version_is_inferred_while_key_sheets_wait_for_confirmation() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = write_stack(&tmp.path().join("scans"));
    let mut inputs = only(&scans, &["a", "b", "blank", "unclear"]);
    // Key sheets for A and B in the stack whose rows are the shared key: the versions look
    // identical, but the key sheets may still change them until they are confirmed.
    let e = iau();
    let shared: Vec<Option<u8>> = (0..N).map(|q| Some(key(0, q))).collect();
    for (k, marker) in ["0000000000", "9999999999"].iter().enumerate() {
        inputs.push(write(
            &tmp.path().join("keys"),
            &format!("key-{k}.png"),
            &png(&sheet(
                &e,
                &Marks::new(&shared, marker, Some(k as u8)),
                90 + k as u64,
            )),
        ));
    }
    let p = grade(
        &identical_exam(&["A", "B"], true),
        &inputs,
        &tmp.path().join("out"),
        2,
    );
    assert!(p.session.key_unconfirmed());
    let mut seen = 0;
    for (bi, b) in p.session.booklets.iter().enumerate() {
        let name = source_of(&p, bi);
        let (stem, shows, _) = SHEETS[sheet_no(&p, bi)];
        if matches!(shows, Shows::Letter(_)) {
            continue;
        }
        seen += 1;
        let sco005: Vec<&str> = b
            .exceptions
            .iter()
            .filter(|e| e.code == "SCO-005")
            .map(|e| e.sub_reason.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(b.form_source, FormSource::Inferred, "{stem} ({name})");
        assert_eq!(sco005, ["inferred"], "{stem}");
    }
    assert_eq!(seen, 2, "blank and unclear");
    p.close().unwrap();
}
