//! The full pipeline on the 143 real IAU sheets (restricted data: never committed, never sent
//! anywhere). Skips unless `OMR_IAU_PDF_DIR` (the five scanner PDFs) and `OMR_IAU_REFERENCE`
//! (the adjudicated `reference.json`) are set. `OMR_E2E_OUT` (a scratch folder) receives the
//! graded exams; `OMR_E2E_REPORT` receives the aggregate report. Only counts are printed and
//! assertion messages carry no student values.
//!
//! Two exams, as in the brief: CS 522 (Full_Batch_A/B, 29 questions) and ARTI 503 (three
//! files, 28 questions), both with `--infer-key` (no keys were supplied).

use omr_app::model::ResponseStatus;
use omr_app::{ExamDefinition, ExamProject, GradeOptions, KeySource, TemplateChoice};
use omr_model::{CancelMode, Response};
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

#[derive(Default, Debug)]
struct Tally {
    pages: usize,
    questions: usize,
    auto: usize,
    silent: usize,
    review: usize,
    id_exact: usize,
    id_flagged: usize,
    id_wrong_unflagged: usize,
    form_declared_correct: usize,
    form_inferred: usize,
    form_wrong: usize,
    /// Pages dropped as blank backs (CAP-012): none of the real pages is blank.
    blank: usize,
    /// Smallest upright-vs-upside-down margin of the coarse correlation (milli).
    orientation_margin_min_milli: i64,
}

fn run_exam(
    pdfs: &[PathBuf],
    questions: u16,
    out: &Path,
    reference: &BTreeMap<(String, u64), Value>,
    t: &mut Tally,
) -> f64 {
    let mut def = ExamDefinition::new(
        "Final",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        questions,
        5,
        vec!["A".into(), "B".into()],
        CancelMode::EraseOnly,
    );
    def.key = KeySource::Inferred;
    let opts = GradeOptions {
        threads: 4,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-16T00:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
        cancel: Default::default(),
    };
    let t0 = std::time::Instant::now();
    let mut p = ExamProject::grade(&def, pdfs, &opts, &|_| {}).expect("grade");
    p.export(out).expect("export");
    let wall = t0.elapsed().as_secs_f64();
    let s = &p.session;
    for pg in &s.pages {
        t.blank += usize::from(pg.status == omr_app::model::PageStatus::Blank);
        if let Some(m) = pg
            .read
            .as_ref()
            .and_then(|r| r.registration["orientation_margin_milli"].as_i64())
        {
            t.orientation_margin_min_milli = t.orientation_margin_min_milli.min(m);
        }
    }
    for b in &s.booklets {
        let pg = &s.pages[b.pages[0]];
        let tag = tag_of(&s.sources[pg.source].name).expect("known file");
        let r = &reference[&(tag.to_string(), u64::from(pg.index) + 1)];
        t.pages += 1;
        for (q, qs) in b.questions.iter().enumerate() {
            t.questions += 1;
            if qs.status == ResponseStatus::Auto {
                t.auto += 1;
                if letter(&qs.current) != r["answers"][q].as_str().unwrap() {
                    t.silent += 1;
                }
            } else {
                t.review += 1;
            }
        }
        let flagged = !b.identity.exceptions.is_empty();
        match (&b.identity.student, flagged) {
            (Some(k), false) if k == r["student_number"].as_str().unwrap() => t.id_exact += 1,
            (_, true) => t.id_flagged += 1,
            _ => t.id_wrong_unflagged += 1,
        }
        let v = r["version"].as_str().unwrap();
        let form = b.form.map(|f| s.definition.forms[f].clone());
        match b.form_source {
            omr_app::model::FormSource::Inferred => t.form_inferred += 1,
            _ if form.as_deref() == Some(v) => t.form_declared_correct += 1,
            _ => t.form_wrong += 1,
        }
    }
    p.close().unwrap();
    wall
}

#[test]
fn req_a1_real_iau_full_pipeline() {
    let (Ok(dir), Ok(refp)) = (
        std::env::var("OMR_IAU_PDF_DIR"),
        std::env::var("OMR_IAU_REFERENCE"),
    ) else {
        eprintln!("skipped: OMR_IAU_PDF_DIR / OMR_IAU_REFERENCE not set");
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
    // The students' PDFs only (the folder also holds the instructors' key sheets).
    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .filter(|p| tag_of(&p.to_string_lossy()).is_some())
        .collect();
    pdfs.sort();
    let (cs, arti): (Vec<PathBuf>, Vec<PathBuf>) = pdfs
        .into_iter()
        .partition(|p| p.to_string_lossy().contains("Full_Batch"));
    let scratch = std::env::var("OMR_E2E_OUT").map(PathBuf::from);
    let tmp = tempfile::tempdir().unwrap();
    let root = scratch.unwrap_or_else(|_| tmp.path().to_path_buf());
    let _ = std::fs::remove_dir_all(root.join("test-cs522"));
    let _ = std::fs::remove_dir_all(root.join("test-arti503"));
    let mut t = Tally {
        orientation_margin_min_milli: i64::MAX,
        ..Tally::default()
    };
    let w1 = run_exam(&cs, 29, &root.join("test-cs522"), &reference, &mut t);
    let w2 = run_exam(&arti, 28, &root.join("test-arti503"), &reference, &mut t);
    let report = json!({
        "pages": t.pages, "questions": t.questions, "auto_accepted": t.auto,
        "silent_errors": t.silent, "review": t.review,
        "ar": t.review as f64 / t.questions as f64,
        "student_number": {"exact": t.id_exact, "flagged": t.id_flagged, "wrong_unflagged": t.id_wrong_unflagged},
        "form": {"declared_correct": t.form_declared_correct, "inferred": t.form_inferred, "wrong": t.form_wrong},
        "blank_pages": t.blank, "orientation_margin_min": t.orientation_margin_min_milli as f64 / 1000.0,
        "wall_s": w1 + w2, "pages_per_s": t.pages as f64 / (w1 + w2),
    });
    eprintln!("{report}");
    if let Ok(p) = std::env::var("OMR_E2E_REPORT") {
        std::fs::write(p, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert_eq!(t.pages, 143);
    assert_eq!(t.silent, 0, "silent errors");
    assert!((t.review as f64) / (t.questions as f64) <= 0.005, "AR");
    assert_eq!(t.id_wrong_unflagged, 0, "wrong unflagged student numbers");
    assert_eq!(t.form_wrong, 0, "wrong declared forms");
    assert_eq!(t.blank, 0, "no real sheet is a blank back");
}
