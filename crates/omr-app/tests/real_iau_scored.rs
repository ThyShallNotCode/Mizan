//! Scoring of the two real IAU exams with the instructors' real answer keys (restricted data:
//! the scans are personal data and the keys exam-confidential; nothing is committed, printed or
//! sent anywhere). Skips unless `OMR_IAU_PDF_DIR` (the scanner PDFs and the key-sheet PDFs),
//! `OMR_IAU_REFERENCE` (the adjudicated `reference.json`) and `OMR_IAU_KEYS` (the folder with
//! `key_cs522.csv` and `key_arti503.csv`) are set. Only counts are printed; assertion messages
//! carry no student values and no key letters. `OMR_E2E_OUT` receives the graded exams;
//! the aggregate report goes to `OMR_SCORED_REPORT`, else beside `OMR_E2E_REPORT` as
//! `real-scored.json`.
//!
//! Proves on the real exams (CS 522: 92 sheets, 29 questions; ARTI 503: 51 sheets, 28):
//! - every Scored booklet's raw score equals an independent score (the reference answers
//!   against the key, computed here from `reference.json`);
//! - every Provisional booklet differs only by questions still pending review (the app never
//!   credits a pending question);
//! - no auto-accepted response differs from the reference;
//! - no form is wrong where the reference shows a version;
//! - grading with the keys read from the instructors' key sheets (`--key-sheet`, confirmed)
//!   gives byte-identical decision files; a key sheet that is not read correctly is refused,
//!   never a wrong key.

use omr_app::keysheet::RowStatus;
use omr_app::model::ResponseStatus;
use omr_app::{ExamDefinition, ExamProject, FormKey, GradeOptions, KeySource, TemplateChoice};
use omr_model::{CancelMode, Response};
use omr_scoring::AssessmentStatus;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn tag_of(name: &str) -> Option<&'static str> {
    [
        ("Full_Batch_A", "BA"),
        ("Full_Batch_B", "BB"),
        ("AI1_ARTI503_-_Final_MCQ_A", "AI1A"),
        ("AI1_ARTI503_-_Final_MCQ_B", "AI1B"),
        ("MS1_ARTI503_-_Final_MCQ_B", "MS1B"),
    ]
    .iter()
    .find(|(p, _)| name.contains(p))
    .map(|(_, t)| *t)
}

fn letter(r: &Response) -> String {
    match r {
        Response::Single(o) => char::from(b'A' + o).to_string(),
        Response::Blank => "BLANK".into(),
        Response::Multi(v) | Response::Set(v) => {
            format!(
                "MULTI:{}",
                v.iter().map(|o| char::from(b'A' + o)).collect::<String>()
            )
        }
        Response::Unreadable { .. } => "UNREADABLE".into(),
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

fn definition(questions: u16, key: KeySource) -> ExamDefinition {
    let mut def = ExamDefinition::new(
        "Final",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        questions,
        5,
        vec!["A".into(), "B".into()],
        CancelMode::EraseOnly,
    );
    def.key = key;
    def.validate().expect("definition");
    def
}

fn options(out: &Path) -> GradeOptions {
    GradeOptions {
        threads: 4,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-17T00:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
        cancel: Default::default(),
    }
}

#[derive(Default, Debug)]
struct Tally {
    booklets: usize,
    scored: usize,
    scored_equal: usize,
    provisional: usize,
    provisional_pending_only: usize,
    other_status: usize,
    pending_questions: usize,
    auto: usize,
    auto_differ: usize,
    form_checked: usize,
    form_wrong: usize,
    form_reference_blank: usize,
}

struct Exam {
    name: &'static str,
    questions: u16,
    students: Vec<PathBuf>,
    key_sheets: Vec<PathBuf>,
    key: Vec<FormKey>,
}

/// Checks a graded exam against the reference and the key; returns the aggregate statistics.
fn check(
    p: &ExamProject,
    key: &[FormKey],
    reference: &BTreeMap<(String, u64), Value>,
    t: &mut Tally,
) -> Value {
    let s = &p.session;
    let sc = p.scoring.as_ref().expect("scored");
    let run = sc.run.as_ref().expect("score run");
    let grades = sc.grades();
    let mut passing = 0usize;
    let mut pct_sum = 0.0f64;
    for (bi, b) in s.booklets.iter().enumerate() {
        let pg = &s.pages[b.pages[0]];
        let tag = tag_of(&s.sources[pg.source].name).expect("a student file");
        let r = &reference[&(tag.to_string(), u64::from(pg.index) + 1)];
        t.booklets += 1;
        let f = b.form.expect("every booklet has a form");
        let v = r["version"].as_str().unwrap_or("");
        if v == "A" || v == "B" {
            t.form_checked += 1;
            t.form_wrong += usize::from(s.definition.forms[f] != v);
        } else {
            t.form_reference_blank += 1;
        }
        let k = &key[f].answers;
        let mut all = 0i128;
        let mut settled = 0i128;
        let mut pending = 0usize;
        for (q, qs) in b.questions.iter().enumerate() {
            let truth = r["answers"][q].as_str().unwrap_or("");
            let credit = i128::from(truth == k[q].as_str());
            all += credit;
            if qs.status == ResponseStatus::Pending {
                pending += 1;
            } else {
                settled += credit;
            }
            if qs.status == ResponseStatus::Auto {
                t.auto += 1;
                t.auto_differ += usize::from(letter(&qs.current) != truth);
            }
        }
        t.pending_questions += pending;
        let row = sc.row_of[bi].expect("a scored row");
        let res = &run.students[row];
        match res.status {
            AssessmentStatus::Scored => {
                t.scored += 1;
                t.scored_equal += usize::from(res.raw == omr_scoring::exact(all, 1));
                if grades
                    .get(row)
                    .and_then(|g| g.as_ref())
                    .is_some_and(|g| g.is_passing())
                {
                    passing += 1;
                }
                pct_sum += 100.0 * all as f64 / f64::from(s.definition.questions);
            }
            AssessmentStatus::Provisional => {
                t.provisional += 1;
                t.provisional_pending_only +=
                    usize::from(pending > 0 && res.raw == omr_scoring::exact(settled, 1));
            }
            _ => t.other_status += 1,
        }
    }
    // The item analysis the exports carry: flag stability (§12.2) merged.
    let a = p.analytics_with_stability().expect("item analysis");
    let st = a.stability.as_ref().expect("flag stability");
    let test = a.tests.first().expect("test statistics");
    let scored = s
        .booklets
        .iter()
        .enumerate()
        .filter(|(bi, _)| {
            sc.row_of[*bi].is_some_and(|r| run.students[r].status == AssessmentStatus::Scored)
        })
        .count();
    json!({
        "booklets": s.booklets.len(),
        "kr20": test.kr20,
        "mean_raw": test.summary.mean,
        "sd_raw": test.summary.sd,
        "mean_percent_scored": if scored == 0 { 0.0 } else { pct_sum / scored as f64 },
        "pass_rate_scored": if scored == 0 { 0.0 } else { passing as f64 / scored as f64 },
        "mis_key_flags": a.flags.iter().filter(|f| f.rule == omr_analytics::Rule::ProbableMiskey).count(),
        "mis_key_flags_low_confidence": a.flags.iter().filter(|f| {
            f.rule == omr_analytics::Rule::ProbableMiskey
                && f.confidence != omr_analytics::Confidence::Normal
        }).count(),
        "stability_method": format!("{:?}", st.method),
        "stability_resamples": st.resamples,
        "rank_1_4_flags": st.flags.len(),
        "rank_1_4_flags_low_confidence": st.flags.iter().filter(|f| f.low_confidence).count(),
    })
}

fn decision_files_identical(a: &Path, b: &Path) -> usize {
    DECISION_FILES
        .iter()
        .filter(|f| {
            let x = std::fs::read(a.join(f)).expect("decision file");
            let y = std::fs::read(b.join(f)).expect("decision file");
            x == y
        })
        .count()
}

#[test]
fn req_a8_real_iau_exams_score_with_the_real_keys() {
    let (Ok(dir), Ok(refp), Ok(keys)) = (
        std::env::var("OMR_IAU_PDF_DIR"),
        std::env::var("OMR_IAU_REFERENCE"),
        std::env::var("OMR_IAU_KEYS"),
    ) else {
        eprintln!("skipped: OMR_IAU_PDF_DIR / OMR_IAU_REFERENCE / OMR_IAU_KEYS not set");
        return;
    };
    let reference: BTreeMap<(String, u64), Value> =
        serde_json::from_slice::<Vec<Value>>(&std::fs::read(&refp).unwrap())
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    (
                        r["tag"].as_str().unwrap().to_string(),
                        r["page"].as_u64().unwrap(),
                    ),
                    r,
                )
            })
            .collect();
    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    pdfs.sort();
    let read_key = |name: &str, n: u16| -> Vec<FormKey> {
        let bytes = std::fs::read(Path::new(&keys).join(name)).expect("key file");
        let (mut forms, _) =
            omr_app::exam::read_key_file(&bytes, &["A".into(), "B".into()], n, 5).expect("key");
        // Normalised entries (`B`), as the key summary shows them.
        for f in &mut forms {
            for a in &mut f.answers {
                let c: omr_app::model::KeyCell = omr_app::exam::parse_key_entry(a, 5)
                    .expect("key entry")
                    .into();
                *a = c.text();
            }
        }
        forms.sort_by(|x, y| x.form.cmp(&y.form));
        forms
    };
    let named = |pat: &str| -> Vec<PathBuf> {
        pdfs.iter()
            .filter(|p| p.to_string_lossy().contains(pat))
            .cloned()
            .collect()
    };
    let students = |cs: bool| -> Vec<PathBuf> {
        pdfs.iter()
            .filter(|p| {
                tag_of(&p.to_string_lossy()).is_some()
                    && p.to_string_lossy().contains("Full_Batch") == cs
            })
            .cloned()
            .collect()
    };
    let exams = [
        Exam {
            name: "cs522",
            questions: 29,
            students: students(true),
            key_sheets: named("Key_-_A__B"),
            key: read_key("key_cs522.csv", 29),
        },
        Exam {
            name: "arti503",
            questions: 28,
            students: students(false),
            key_sheets: named("ARTI-503_Final_"),
            key: read_key("key_arti503.csv", 28),
        },
    ];
    let tmp = tempfile::tempdir().unwrap();
    let root = std::env::var("OMR_E2E_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    let mut report = serde_json::Map::new();
    let mut t = Tally::default();
    for e in &exams {
        assert!(
            !e.students.is_empty() && !e.key_sheets.is_empty(),
            "{}",
            e.name
        );
        let a = root.join(format!("scored-{}", e.name));
        let b = root.join(format!("scored-{}-key-sheets", e.name));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
        // 1. With the key file.
        let def = definition(
            e.questions,
            KeySource::Provided {
                forms: e.key.clone(),
            },
        );
        let mut p = ExamProject::grade(&def, &e.students, &options(&a), &|_| {}).expect("grade");
        p.export(&a).expect("export");
        let stats = check(&p, &e.key, &reference, &mut t);
        p.close().unwrap();
        // 2. With the key sheets (named), confirmed by the instructor.
        let def = definition(e.questions, KeySource::None);
        let mut q = ExamProject::grade_with_key_sheets(
            &def,
            &e.students,
            &e.key_sheets,
            &options(&b),
            &|_| {},
        )
        .expect("grade with key sheets");
        assert_eq!(
            q.session.booklets.len(),
            stats["booklets"].as_u64().unwrap() as usize
        );
        let sum = q.key_sheet_summary(false).expect("key sheets");
        let (mut equal, mut wrong_read, mut open) = (0usize, 0usize, 0usize);
        for v in &sum.versions {
            let f = usize::from(v.version == "B");
            for r in &v.rows {
                let truth = &e.key[f].answers[r.question as usize - 1];
                match r.status {
                    RowStatus::Read if r.answer == *truth => equal += 1,
                    RowStatus::Read => wrong_read += 1,
                    _ => open += 1,
                }
            }
        }
        let refused = sum.unusable.len();
        // Never a wrong key: a row read confidently is the instructor's answer.
        assert_eq!(wrong_read, 0, "{}: key-sheet rows read wrongly", e.name);
        let all = 2 * usize::from(e.questions);
        let read_all = sum.versions.len() == 2 && equal == all && open == 0 && refused == 0;
        let mut identical = None;
        if read_all {
            let r = q
                .confirm_key_sheets(None, Vec::new(), None)
                .expect("confirm");
            assert_eq!(r.key_version, Some(1));
            q.export(&b).expect("export");
            identical = Some(decision_files_identical(&a, &b));
        } else {
            // Not read as the key: held, nothing scored, the sheets not used.
            assert!(
                q.session.key_unconfirmed() && q.scoring.is_none(),
                "{}",
                e.name
            );
        }
        q.close().unwrap();
        // 3. The key sheets inside the students' stack, found by the rule (all-0 / all-9
        //    student number and a version bubble), confirmed.
        let c = root.join(format!("scored-{}-in-stack", e.name));
        let _ = std::fs::remove_dir_all(&c);
        let mut stack = e.students.clone();
        stack.extend(e.key_sheets.iter().cloned());
        let mut w = ExamProject::grade(&def, &stack, &options(&c), &|_| {}).expect("grade stack");
        let found = w
            .session
            .key_sheets
            .sheets
            .iter()
            .filter(|k| !k.declared && k.usable())
            .count();
        let mut in_stack_identical = None;
        assert_eq!(
            w.session.booklets.len(),
            stats["booklets"].as_u64().unwrap() as usize
        );
        if read_all {
            assert_eq!(found, 2, "{}: key sheets found in the stack", e.name);
            w.confirm_key_sheets(None, Vec::new(), None)
                .expect("confirm");
            w.export(&c).expect("export");
            in_stack_identical = Some(decision_files_identical(&a, &c));
        }
        w.close().unwrap();
        let mut st = stats;
        st["key_sheets"] = json!({
            "sheets": sum.versions.iter().map(|v| v.sheets.len()).sum::<usize>() + refused,
            "versions_read": sum.versions.len(),
            "rows_equal_to_key": equal,
            "rows_read_wrong": wrong_read,
            "rows_to_decide": open,
            "sheets_refused": refused,
            "decision_files_identical": identical.map(|n| format!("{n}/{}", DECISION_FILES.len())),
            "found_in_stack": found,
            "in_stack_decision_files_identical":
                in_stack_identical.map(|n| format!("{n}/{}", DECISION_FILES.len())),
        });
        report.insert(e.name.into(), st);
        if e.name == "arti503" {
            // The 600-dpi scans of the ARTI 503 key sheets read exactly the key.
            assert!(
                read_all,
                "arti503 key sheets: {equal} of {all} rows read as the key"
            );
            assert_eq!(
                identical,
                Some(DECISION_FILES.len()),
                "arti503 decision files"
            );
            assert_eq!(
                in_stack_identical,
                Some(DECISION_FILES.len()),
                "arti503 in the stack"
            );
        } else if read_all {
            assert_eq!(
                identical,
                Some(DECISION_FILES.len()),
                "cs522 decision files"
            );
            assert_eq!(
                in_stack_identical,
                Some(DECISION_FILES.len()),
                "cs522 in the stack"
            );
        }
    }
    let summary = json!({
        "booklets": t.booklets,
        "scored": t.scored,
        "scored_equal_independent": t.scored_equal,
        "provisional": t.provisional,
        "provisional_differ_only_by_pending": t.provisional_pending_only,
        "other_status": t.other_status,
        "pending_questions": t.pending_questions,
        "auto_accepted": t.auto,
        "auto_accepted_differing_from_reference": t.auto_differ,
        "forms_checked": t.form_checked,
        "forms_wrong": t.form_wrong,
        "forms_reference_blank": t.form_reference_blank,
        "exams": Value::Object(report),
    });
    eprintln!("{}", serde_json::to_string_pretty(&summary).unwrap());
    let out = std::env::var("OMR_SCORED_REPORT")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("OMR_E2E_REPORT")
                .ok()
                .and_then(|r| Path::new(&r).parent().map(|d| d.join("real-scored.json")))
        });
    if let Some(o) = out {
        std::fs::write(o, serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
    }
    assert_eq!(t.booklets, 143);
    assert_eq!(t.other_status, 0, "booklets neither scored nor provisional");
    assert_eq!(
        t.scored_equal, t.scored,
        "scored booklets differing from the independent score"
    );
    assert_eq!(
        t.provisional_pending_only, t.provisional,
        "provisional booklets differing beyond their pending questions"
    );
    assert_eq!(
        t.auto_differ, 0,
        "auto-accepted responses differing from the reference"
    );
    assert_eq!(
        t.form_wrong, 0,
        "forms wrong where the reference has a version"
    );
}
