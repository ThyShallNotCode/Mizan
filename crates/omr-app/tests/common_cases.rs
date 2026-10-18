//! Common cases found in review, proven on synthetic IAU sheets only:
//! - C11 a four-option exam on the five-option IAU sheet: a filled E never aborts the run and
//!   is never dropped; it is queued as MRK-017 `option_not_offered` without a proposal, and an
//!   inferred key never names E (CR-02);
//! - C12 roster students who handed in no sheet are listed `absent` (not 0) in the results and
//!   counted in the summary and the view-model, never in the Blackboard upload (CR-03);
//! - C13 the section bubbled on the sheet is carried: it fills the Section column of a
//!   rosterless exam and is checked against the roster (IDN-004 `cross_section`, INFO) (CR-10);
//! - C14 a folder holding the start of a grading run that did not finish says so, with the
//!   remedy (CR-13).

mod common;

use common::*;
use omr_app::{Action, Decision, ExamProject, KeySource, RosterStudent};
use omr_forms::legacy::LegacyField;
use std::path::Path;

const N: u16 = 10;

fn student(k: u16) -> String {
    format!("22106789{k:02}")
}

fn key4(q: u16) -> u8 {
    (q % 4) as u8
}

/// Sheet k: the key's answer everywhere, but E on question k (k < 3), a section bubbled.
fn scans(dir: &Path, sheets: u16, section: impl Fn(u16) -> &'static str) -> std::path::PathBuf {
    let e = iau();
    let out = dir.join("scans");
    for k in 0..sheets {
        let answers: Vec<Option<u8>> = (0..N)
            .map(|q| Some(if q == k && k < 3 { 4 } else { key4(q) }))
            .collect();
        let mut m = Marks::new(&answers, &student(k), Some(0));
        for (c, d) in section(k).bytes().enumerate() {
            m.extra
                .push(bubble(&e, LegacyField::Section, c as u16, d - b'0'));
        }
        write(
            &out,
            &format!("s{k:02}.png"),
            &png(&sheet(&e, &m, 70 + u64::from(k))),
        );
    }
    out
}

fn def4(key: KeySource) -> omr_app::ExamDefinition {
    let mut d = omr_app::ExamDefinition::new(
        "Quiz 4",
        omr_app::TemplateChoice::Legacy("iau-ccsit-v1".into()),
        N,
        4,
        vec!["A".into()],
        omr_model::CancelMode::EraseOnly,
    );
    d.key = key;
    d.validate().unwrap();
    d
}

fn provided() -> KeySource {
    KeySource::Provided {
        forms: vec![omr_app::FormKey {
            form: "A".into(),
            answers: (0..N)
                .map(|q| char::from(b'A' + key4(q)).to_string())
                .collect(),
        }],
    }
}

fn roster(n: u16, section: &str) -> Vec<RosterStudent> {
    (0..n)
        .map(|k| RosterStudent {
            id: student(k),
            name: format!("Student {k}"),
            name_ar: String::new(),
            section: section.into(),
            numeric_cell: false,
        })
        .collect()
}

#[test]
fn c11_four_option_exam_on_five_option_sheet() {
    let d = tempfile::tempdir().unwrap();
    let input = scans(d.path(), 4, |_| "101");
    // With the key: graded, every E queued, nothing aborted.
    let mut p = grade(
        &def4(provided()),
        std::slice::from_ref(&input),
        &d.path().join("k"),
        2,
    );
    let e_items: Vec<_> = p
        .queue(None)
        .into_iter()
        .filter(|i| i.sub_reason.as_deref() == Some("option_not_offered"))
        .map(|i| (i.id, i.code.clone(), i.suggestion.clone(), i.question))
        .collect();
    assert_eq!(e_items.len(), 3, "one per filled E");
    for (_, code, sugg, _) in &e_items {
        assert_eq!(code, "MRK-017");
        assert_eq!(
            *sugg, None,
            "no proposal names an option the exam does not use"
        );
    }
    assert!(p.scoring.as_ref().unwrap().run.is_ok());
    assert!(p.session.release_blocked().unwrap().contains("ERROR"));
    // A reviewer records "no valid answer" (blank): scored, no reason needed.
    let (id, ..) = e_items[0];
    p.apply(
        id,
        Action::Resolve {
            decision: Decision::Blank,
            accepted_suggestion: false,
            note: String::new(),
        },
    )
    .unwrap();
    // An E is never an option of the exam.
    let e = p
        .apply(
            e_items[1].0,
            Action::Resolve {
                decision: Decision::Option(4),
                accepted_suggestion: false,
                note: String::new(),
            },
        )
        .unwrap_err();
    assert!(e.to_string().contains("4 options"), "{e}");
    p.close().unwrap();
    // With an inferred key: graded, and the key never names E.
    let p = grade(&def4(KeySource::Inferred), &[input], &d.path().join("i"), 2);
    let k = p.session.key.as_ref().unwrap();
    assert!(k.forms[0].iter().all(|c| c.text() != "E"));
    assert!(p.scoring.as_ref().unwrap().run.is_ok());
    p.close().unwrap();
}

#[test]
fn c12_absent_roster_students_are_listed_absent() {
    let d = tempfile::tempdir().unwrap();
    let input = scans(d.path(), 3, |_| "101");
    let mut def = def4(provided());
    def.roster = Some(roster(5, "101"));
    def.validate().unwrap();
    let out = d.path().join("out");
    let p = grade(&def, &[input], &out, 2);
    let absent: Vec<&str> = p.absent_students().iter().map(|r| r.id.as_str()).collect();
    assert_eq!(absent, vec![student(3).as_str(), student(4).as_str()]);
    let csv = std::fs::read_to_string(out.join("results.csv")).unwrap();
    let rows: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(rows.len(), 5, "{csv}");
    let absent_rows: Vec<&&str> = rows.iter().filter(|r| r.contains(",absent,")).collect();
    assert_eq!(absent_rows.len(), 2, "{csv}");
    for r in &absent_rows {
        // No booklet, no score: never a 0.
        assert!(r.contains(",,,absent,"), "{r}");
        assert!(!r.contains("0.00"), "{r}");
    }
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["absent"], 2);
    assert_eq!(p.results_vm().unwrap().absent, 2);
    if let Ok(bb) = std::fs::read_to_string(out.join("blackboard_grade_center.csv")) {
        assert!(!bb.contains(&student(3)) && !bb.contains(&student(4)));
    }
    p.close().unwrap();
}

#[test]
fn c13_sheet_section_is_carried_and_checked_against_the_roster() {
    let d = tempfile::tempdir().unwrap();
    let input = scans(d.path(), 3, |k| if k == 2 { "202" } else { "101" });
    // Rosterless: the Section column comes from the sheets.
    let out = d.path().join("rl");
    let p = grade(&def4(provided()), std::slice::from_ref(&input), &out, 2);
    let mut secs: Vec<&str> = p
        .session
        .booklets
        .iter()
        .map(|b| b.sheet_section.as_str())
        .collect();
    secs.sort();
    assert_eq!(secs, vec!["101", "101", "202"]);
    let csv = std::fs::read_to_string(out.join("results.csv")).unwrap();
    assert!(csv.contains("\"202\""), "{csv}");
    p.close().unwrap();
    // With a roster that puts everyone in 101: the 202 sheet is surfaced, not queued.
    let mut def = def4(provided());
    def.roster = Some(roster(3, "101"));
    let p = grade(&def, &[input], &d.path().join("r"), 2);
    let cross: Vec<_> = p
        .session
        .booklets
        .iter()
        .flat_map(|b| &b.identity.exceptions)
        .filter(|e| e.sub_reason.as_deref() == Some("cross_section"))
        .collect();
    assert_eq!(cross.len(), 1);
    assert_eq!(cross[0].severity, omr_model::Severity::Info);
    assert!(
        p.queue(None)
            .iter()
            .all(|i| i.sub_reason.as_deref() != Some("cross_section"))
    );
    p.close().unwrap();
}

#[test]
fn c14_an_unfinished_grading_run_is_named_with_its_remedy() {
    let d = tempfile::tempdir().unwrap();
    let pkg = d.path().join("out").join("project.omrproj");
    let st = omr_store::Project::create(
        &pkg,
        omr_store::NewProject::default(),
        omr_store::OpenOptions {
            check_location: false,
            ..omr_store::OpenOptions::default()
        },
    )
    .unwrap();
    st.close().unwrap();
    let e = ExamProject::open(&pkg, "principal:test").unwrap_err();
    assert_eq!(e.exit_code(), 7);
    let t = e.to_string();
    assert!(
        t.contains("did not finish") && t.contains("delete the folder"),
        "{t}"
    );
}
