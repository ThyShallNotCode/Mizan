//! Remark import (spec §3.5) and the parallel run (§23.7) on the two real IAU exams, weighted
//! grading with the points imported from Remark's own graded workbooks (restricted data: the
//! scans and Remark's exports are personal data, the keys exam-confidential; nothing is
//! committed, printed or sent anywhere). Skips unless `OMR_REMARK_DIR` (Remark's graded
//! `.xlsx` exports), `OMR_IAU_PDF_DIR` (the scanner PDFs), `OMR_IAU_REFERENCE` (the adjudicated
//! `reference.json`) and `OMR_IAU_KEYS` (the verified `key_cs522.csv`, `key_arti503.csv`) are
//! set. `OMR_E2E_OUT` receives the graded exams; `OMR_REMARK_REPORT` receives the internal
//! comparison reports and the counts of each exam (Restricted: head-to-head figures stay inside
//! the university, spec §3.5, Q-08). Nothing else is printed; assertion messages carry no
//! student value, no key letter and no head-to-head figure.
//!
//! Committed here are only invariants that hold for any exam: the keys imported from Remark's
//! workbooks equal the verified key files; the imported points reproduce every Remark total
//! exactly (points the totals leave undetermined are given as the import suggests, never
//! assumed by it); an exam graded with the imported key and points shows no key, points or
//! maximum difference; the report carries its banner. What the parallel run on these exams
//! found (identity differences, key sheets, totals) is checked against `OMR_REMARK_EXPECT`, a
//! restricted JSON file kept outside the repository with the other restricted data: for every
//! exam and every count it names, the run must give that count.

use omr_app::exam::{FormKey, parse_key_entry, read_key_file};
use omr_app::model::KeyCell;
use omr_app::parallel::{CompareOptions, HandwrittenId, RESTRICTED_BANNER};
use omr_app::remark::{GivenPoints, import, read_workbook_files, suggest_points};
use omr_app::{ExamDefinition, ExamProject, GradeOptions, KeySource, TemplateChoice};
use omr_model::CancelMode;
use omr_scoring::Exact;
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

fn normalised(forms: Vec<FormKey>) -> Vec<FormKey> {
    let mut forms: Vec<FormKey> = forms
        .into_iter()
        .map(|mut f| {
            for a in &mut f.answers {
                let c: KeyCell = parse_key_entry(a, 5).expect("key entry").into();
                *a = c.text();
            }
            f
        })
        .collect();
    forms.sort_by(|x, y| x.form.cmp(&y.form));
    forms
}

struct Exam {
    name: &'static str,
    questions: u16,
    students: Vec<PathBuf>,
    workbooks: Vec<PathBuf>,
    key_file: &'static str,
}

#[test]
fn req_a8_real_exams_graded_with_remark_points_and_compared_with_remark() {
    let (Ok(remark), Ok(dir), Ok(refp), Ok(keys)) = (
        std::env::var("OMR_REMARK_DIR"),
        std::env::var("OMR_IAU_PDF_DIR"),
        std::env::var("OMR_IAU_REFERENCE"),
        std::env::var("OMR_IAU_KEYS"),
    ) else {
        eprintln!(
            "skipped: OMR_REMARK_DIR / OMR_IAU_PDF_DIR / OMR_IAU_REFERENCE / OMR_IAU_KEYS not set"
        );
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
    let list = |d: &str, ext: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == ext))
            .collect();
        v.sort();
        v
    };
    let pdfs = list(&dir, "pdf");
    let xlsx = list(&remark, "xlsx");
    let students = |cs: bool| -> Vec<PathBuf> {
        pdfs.iter()
            .filter(|p| {
                tag_of(&p.to_string_lossy()).is_some()
                    && p.to_string_lossy().contains("Full_Batch") == cs
            })
            .cloned()
            .collect()
    };
    let books = |pat: &str| -> Vec<PathBuf> {
        xlsx.iter()
            .filter(|p| {
                let n = p.to_string_lossy();
                n.contains(pat) && n.contains("Graded")
            })
            .cloned()
            .collect()
    };
    // Handwritten numbers (the reference's reading of the written ID).
    let names = [
        ("BA", "Full_Batch_A"),
        ("BB", "Full_Batch_B"),
        ("AI1A", "AI1_ARTI503_-_Final_MCQ_A"),
        ("AI1B", "AI1_ARTI503_-_Final_MCQ_B"),
        ("MS1B", "MS1_ARTI503_-_Final_MCQ_B"),
    ];
    let handwritten: Vec<HandwrittenId> = reference
        .values()
        .filter_map(|r| {
            let tag = r["tag"].as_str()?;
            let file = names.iter().find(|(t, _)| *t == tag)?.1;
            let ids: Vec<String> = r["student_id_handwritten"]
                .as_array()?
                .iter()
                .filter_map(|h| h.as_str().map(|s| s.replace(' ', "")))
                .filter(|s| !s.is_empty())
                .collect();
            let page = r["page"].as_u64()? as u32;
            (!ids.is_empty()).then(|| HandwrittenId {
                file: file.into(),
                page,
                ids,
            })
        })
        .collect();
    let exams = [
        Exam {
            name: "cs522",
            questions: 29,
            students: students(true),
            workbooks: books("Graded_All_Sections"),
            key_file: "key_cs522.csv",
        },
        Exam {
            name: "arti503",
            questions: 28,
            students: students(false),
            workbooks: books("ARTI503"),
            key_file: "key_arti503.csv",
        },
    ];
    let tmp = tempfile::tempdir().unwrap();
    let root = std::env::var("OMR_E2E_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    let report_root = std::env::var("OMR_REMARK_REPORT").ok().map(PathBuf::from);
    // The restricted expectations of the internal parallel run (never in the repository).
    let expect: Option<Value> = std::env::var("OMR_REMARK_EXPECT")
        .ok()
        .map(|f| serde_json::from_slice(&std::fs::read(f).expect("expectations")).unwrap());
    let mut checked = 0usize;
    for e in &exams {
        assert!(!e.students.is_empty(), "{}: scans", e.name);
        assert!(!e.workbooks.is_empty(), "{}: Remark workbooks", e.name);
        let wbs = read_workbook_files(&e.workbooks).expect("workbooks");
        // 1. Import: points without help where the workbooks determine them.
        let first = import(&wbs, &GivenPoints::new());
        let suggested = suggest_points(&wbs, &GivenPoints::new()).expect("suggest");
        let given_questions: usize = suggested.values().map(|v| v.iter().flatten().count()).sum();
        let imp = match first {
            Ok(imp) => {
                assert_eq!(given_questions, 0, "{}", e.name);
                imp
            }
            Err(err) => {
                let m = err.to_string();
                assert!(m.contains("do not determine"), "{}: refusal", e.name);
                import(&wbs, &suggested).expect("import with the suggested points")
            }
        };
        // The keys equal the verified key files.
        let bytes = std::fs::read(Path::new(&keys).join(e.key_file)).expect("key file");
        let (verified, _) =
            read_key_file(&bytes, &["A".into(), "B".into()], e.questions, 5).expect("key");
        assert_eq!(
            normalised(imp.form_keys()),
            normalised(verified),
            "{}: imported keys differ from the verified ones",
            e.name
        );
        // Every Remark total reproduced.
        for v in &imp.versions {
            assert_eq!(v.totals_reproduced, v.rows_checked, "{} {}", e.name, v.form);
            assert_eq!(
                v.points.iter().sum::<Exact>(),
                v.total_possible,
                "{} {}",
                e.name,
                v.form
            );
        }
        let mut pa = imp.version("A").unwrap().points.clone();
        let mut pb = imp.version("B").unwrap().points.clone();
        pa.sort();
        pb.sort();
        // 2. Grade with the imported key, points and (ARTI 503) learning objectives.
        let mut def = ExamDefinition::new(
            "Final",
            TemplateChoice::Legacy("iau-ccsit-v1".into()),
            e.questions,
            5,
            vec!["A".into(), "B".into()],
            CancelMode::EraseOnly,
        );
        def.key = KeySource::Provided {
            forms: imp.form_keys(),
        };
        def.set_key_points(&imp.key_points());
        def.outcomes = imp.outcome_setup();
        def.validate().expect("definition");
        let out = root.join(format!("remark-{}", e.name));
        let _ = std::fs::remove_dir_all(&out);
        let mut p = ExamProject::grade(&def, &e.students, &options(&out), &|_| {}).expect("grade");
        p.export(&out).expect("export");
        // 3. The parallel run.
        let opts = CompareOptions {
            given_points: if given_questions > 0 {
                suggested.clone()
            } else {
                GivenPoints::new()
            },
            handwritten: handwritten.clone(),
            crops: report_root.is_some(),
        };
        let run = p.compare_remark(&wbs, &opts).expect("compare");
        let c = &run.counts;
        // Graded with the imported key and points: nothing to tell apart but the readings.
        assert_eq!(c.key_differences, 0, "{}: key differences", e.name);
        assert!(
            run.points_differences.is_empty() && run.maximum_differences.is_empty(),
            "{}: the exam carries the imported points",
            e.name
        );
        for st in run.students.iter().filter(|st| st.remark.is_some()) {
            assert!(st.graded, "{}: every paired booklet graded", e.name);
        }
        // Provisional totals resolved as the reference reads their pending questions.
        let s = &p.session;
        let mut provisional_ok = 0usize;
        let mut provisional = 0usize;
        let mut pending_remark_vs_reference = 0usize;
        for st in run.students.iter().filter(|st| !st.settled) {
            let Some(rt) = st.remark else { continue };
            provisional += 1;
            let b = &s.booklets[st.booklet];
            let pg = &s.pages[b.pages[0]];
            let tag = tag_of(&s.sources[pg.source].name).expect("student file");
            let r = &reference[&(tag.to_string(), u64::from(pg.index) + 1)];
            let v = imp.version(&st.form).expect("version");
            let key: Vec<KeyCell> = v
                .key
                .iter()
                .map(|k| parse_key_entry(k, 5).expect("key").into())
                .collect();
            let mut t = st.ours;
            for &q in &st.pending {
                let q = usize::from(q) - 1;
                let truth = r["answers"][q].as_str().unwrap_or("");
                if key[q].text() == truth {
                    t += v.points[q];
                }
                let pair = run
                    .pairs
                    .iter()
                    .position(|x| x.booklet == st.booklet)
                    .expect("paired");
                let remark = run
                    .questions_rows
                    .iter()
                    .find(|x| x.pair == pair && usize::from(x.question) == q + 1)
                    .expect("question row");
                pending_remark_vs_reference += usize::from(remark.remark != truth);
            }
            provisional_ok += usize::from(t == rt);
        }
        assert_eq!(c.totals_provisional, provisional, "{}", e.name);
        // The statistics Remark reported are reproduced from its rows (key sheets included).
        for k in &run.key_sheets {
            let rep = k.reported.as_ref().expect("Test Statistics Report");
            let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) => (a - b).abs() < 5e-3,
                _ => false,
            };
            assert!(close(rep.mean, k.with.mean), "{}: mean", e.name);
            assert!(close(rep.sd, k.with.sd), "{}: SD", e.name);
            assert!(close(rep.max, k.with.max), "{}: max", e.name);
            assert!(close(rep.kr20, k.with.kr20), "{}: KR-20", e.name);
            assert!(close(rep.alpha, k.with.alpha), "{}: alpha", e.name);
        }
        let effects: Vec<Value> = run
            .key_sheets
            .iter()
            .map(|k| {
                json!({
                    "rows": k.rows.len(),
                    "kr20_inflation": k.with.kr20.zip(k.without.kr20).map(|(a, b)| a - b),
                    "alpha_inflation": k.with.alpha.zip(k.without.alpha).map(|(a, b)| a - b),
                    "mean_inflation": k.with.mean.zip(k.without.mean).map(|(a, b)| a - b),
                    "top_percent_without": k.without.max_percent,
                })
            })
            .collect();
        // The run's counts (Restricted): checked against the expectations, written to the
        // internal report folder, never printed.
        let facts = json!({
            "booklets": c.booklets,
            "remark_rows": c.remark_rows,
            "out_of_scope_rows": c.out_of_scope_rows,
            "key_sheet_rows": c.key_sheet_rows,
            "key_sheet_workbooks": run.key_sheets.len(),
            "key_sheets_inflate_kr20": run.key_sheets.iter().all(|k| {
                k.with.kr20.zip(k.without.kr20).is_some_and(|(a, b)| a > b)
            }),
            "points_b_permutation_of_a": pa == pb,
            "paired_by_id": c.paired_by_id,
            "paired_by_partial_id": c.paired_by_partial_id,
            "paired_by_answers": c.paired_by_answers,
            "questions": c.questions,
            "agree": c.agree,
            "agree_pending": c.agree_pending,
            "differ": c.differ,
            "differ_pending": c.differ_pending,
            "totals_settled": c.totals_settled,
            "totals_settled_equal": c.totals_settled_equal,
            "totals_settled_unequal": c.totals_settled - c.totals_settled_equal,
            "totals_provisional": c.totals_provisional,
            "totals_provisional_unequal_if_remark":
                c.totals_provisional - c.totals_provisional_equal_if_remark,
            "provisional_equal_with_pending_resolved_as_reference": provisional_ok,
            "pending_questions_remark_reads_unlike_reference": pending_remark_vs_reference,
            "answers_only_equal": c.answers_only_equal,
            "not_queued": c.not_queued,
            "possible_key_sheet_rows": c.possible_key_sheet_rows,
            "objective_subtotals": c.objective_subtotals,
            "objective_subtotals_equal": c.objective_subtotals_equal,
            "remark_totals_reproduced": c.remark_totals_reproduced,
            "points_given_questions": given_questions,
            "points_sources": imp.versions.iter()
                .map(|v| v.points_source.describe()).collect::<Vec<_>>(),
            "identity": c.identity,
            "key_sheet_effects": effects,
        });
        if let Some(want) = expect.as_ref().and_then(|x| x.get(e.name)) {
            for (k, v) in want.as_object().expect("an object per exam") {
                assert!(
                    facts.get(k) == Some(v),
                    "{}: `{k}` is not what the restricted expectations say",
                    e.name
                );
                checked += 1;
            }
        }
        // The report: Restricted in every file.
        let report = match &report_root {
            Some(r) => r.join(e.name),
            None => tmp.path().join(format!("report-{}", e.name)),
        };
        p.export_parallel_run(&run, &report).expect("report");
        for f in omr_app::parallel::REPORT_FILES {
            let text = std::fs::read_to_string(report.join(f)).expect("report file");
            assert!(text.contains(RESTRICTED_BANNER), "{}: {f} banner", e.name);
        }
        if let Some(r) = &report_root {
            omr_app::keys::write_private(
                &r.join(format!("{}-summary.json", e.name)),
                &serde_json::to_vec_pretty(&facts).unwrap(),
            )
            .unwrap();
        }
        p.close().unwrap();
    }
    if expect.is_some() {
        assert!(checked > 0, "the expectations name no count of these exams");
    } else {
        eprintln!("OMR_REMARK_EXPECT not set: the parallel run's findings were not checked");
    }
}
