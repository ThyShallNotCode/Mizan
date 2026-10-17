//! End to end on generated sheets (no real data): own-form sheets rendered and "scanned" by
//! `omr-sim` (truth by construction, standard profile, 200-dpi JPEG), graded through the whole
//! pipeline (ingest → recognition → identity → scoring → outcomes → store → exports), reviewed
//! through the API with the truth as the reviewer, and checked against an independent hand
//! computation of every score. Also A5: byte-identical decision files at 1, 2 and 4 threads
//! and in shuffled and reversed input order; A4: rescoring after a review action and after a
//! key change, timed.

use omr_app::exam::{self, CloDefinition, CloMapping, DomainLetter, OutcomeSetup, RosterStudent};
use omr_app::model::ResponseStatus;
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, GradeOptions, IdSchemeChoice, ItemKind,
    KeySource, TemplateChoice,
};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, Response};
use omr_sim::{CorpusConfig, Expected, PageTruth};
use rand::{RngExt, SeedableRng};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const QUESTIONS: u16 = 40;
const OPTIONS: u8 = 4;
const SHEETS: u32 = 24;
const KEY: [u8; 32] = [0x5a; 32];

fn spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid_from(0x0190_aaaa_bbbb_7ccc_8ddd_0000_0000_0077)),
        QUESTIONS,
        OPTIONS,
        CancelMode::EraseOnly,
    );
    s.forms = 2;
    s.title = "Synthetic quiz".into();
    s
}

fn uuid_from(v: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(v)
}

fn roster_ids() -> Vec<String> {
    (0..SHEETS)
        .map(|i| format!("4210{:05}", 100 + i * 7))
        .collect()
}

/// Keys per form (seeded), as letters.
fn keys() -> Vec<Vec<u8>> {
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(99);
    (0..2)
        .map(|_| {
            (0..QUESTIONS)
                .map(|_| rng.random_range(0..OPTIONS))
                .collect()
        })
        .collect()
}

/// Writes the corpus (images + truth) and returns (image folder, truth by serial).
fn corpus(dir: &Path) -> (PathBuf, BTreeMap<u32, PageTruth>) {
    let t = Template::build(spec()).unwrap();
    let mut cfg = CorpusConfig::standard(SHEETS, 20_261_017);
    cfg.roster = roster_ids();
    cfg.threads = 4;
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, dir).unwrap();
    let truth = omr_sim::corpus::read_truth(&dir.join("truth.jsonl")).unwrap();
    let by_serial = truth.into_iter().map(|p| (p.booklet, p)).collect();
    (dir.join("images"), by_serial)
}

fn definition() -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Quiz 1",
        TemplateChoice::Generated(Box::new(spec())),
        QUESTIONS,
        OPTIONS,
        vec!["A".into(), "B".into()],
        CancelMode::EraseOnly,
    );
    d.course_code = "CS 101".into();
    d.page_code_key_hex = Some(hex::encode(KEY));
    let k = keys();
    d.key = KeySource::Provided {
        forms: ["A", "B"]
            .iter()
            .zip(&k)
            .map(|(f, ks)| FormKey {
                form: (*f).into(),
                answers: ks
                    .iter()
                    .map(|&o| char::from(b'A' + o).to_string())
                    .collect(),
            })
            .collect(),
    };
    d.roster = Some(
        roster_ids()
            .into_iter()
            .enumerate()
            .map(|(i, id)| RosterStudent {
                id,
                name: format!("Student {i}"),
                name_ar: String::new(),
                section: if i % 2 == 0 {
                    "101".into()
                } else {
                    "102".into()
                },
                numeric_cell: false,
            })
            .collect(),
    );
    d.id_scheme = IdSchemeChoice::Digits(9);
    d.outcomes = Some(OutcomeSetup {
        clos: vec![
            CloDefinition {
                code: "CLO1".into(),
                title: "Recall".into(),
                title_ar: String::new(),
                domain: DomainLetter::K,
                plo: "K1".into(),
            },
            CloDefinition {
                code: "CLO2".into(),
                title: "Apply".into(),
                title_ar: String::new(),
                domain: DomainLetter::S,
                plo: "S1".into(),
            },
        ],
        mapping: (1..=QUESTIONS)
            .map(|q| CloMapping {
                question: q,
                clo: if q <= 20 {
                    "CLO1".into()
                } else {
                    "CLO2".into()
                },
                weight: "1".into(),
                note: String::new(),
            })
            .collect(),
        split: false,
        policy: exam::PolicyChoice::Default,
    });
    d.exam_id = d.derived_exam_id();
    d.validate().unwrap();
    d
}

fn grade(def: &ExamDefinition, inputs: &[PathBuf], out: &Path, threads: usize) -> ExamProject {
    let opts = GradeOptions {
        threads,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-16T09:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
    };
    let mut p = ExamProject::grade(def, inputs, &opts, &|_| {}).unwrap();
    p.export(out).unwrap();
    p
}

fn decision_of(r: &Response) -> Decision {
    match r {
        Response::Single(o) => Decision::Option(*o),
        Response::Blank => Decision::Blank,
        Response::Multi(v) | Response::Set(v) => Decision::Multi(v.clone()),
        Response::Unreadable { .. } => Decision::Unreadable,
    }
}

/// The reviewed truth of a question: the auto response, or the intended one for review.
fn final_truth(q: &omr_sim::QuestionTruth) -> Response {
    match &q.expected {
        Expected::Auto(r) => r.clone(),
        Expected::Review => q.intended.clone(),
    }
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn req_a1_a8_synthetic_sheets_grade_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let (images, truth) = corpus(&dir.path().join("corpus"));
    let def = definition();
    let out = dir.path().join("out");
    let mut p = grade(&def, std::slice::from_ref(&images), &out, 4);
    let s = &p.session;
    assert_eq!(s.pages.len(), SHEETS as usize);
    assert_eq!(s.booklets.len(), SHEETS as usize);

    // ---- Recognition against the truth (A1): SER = 0, no review-truth auto-accepted.
    let (mut auto, mut silent, mut review_truth_auto, mut reviewed) = (0, 0, 0, 0);
    for b in &s.booklets {
        let t = &truth[&b.serial];
        assert_eq!(
            b.form,
            Some(usize::from(t.form) - 1),
            "booklet {}",
            b.serial
        );
        assert_eq!(b.identity.student.as_deref(), t.student_id.as_deref());
        for (q, qs) in b.questions.iter().enumerate() {
            let qt = &t.questions[q];
            if qs.status == ResponseStatus::Auto {
                auto += 1;
                match &qt.expected {
                    Expected::Auto(r) if *r != qs.current => silent += 1,
                    Expected::Review => review_truth_auto += 1,
                    _ => {}
                }
            } else {
                reviewed += 1;
            }
        }
    }
    let total = usize::from(QUESTIONS) * SHEETS as usize;
    let ar = reviewed as f64 / total as f64;
    eprintln!("auto {auto}, silent {silent}, review-truth auto {review_truth_auto}, AR {ar:.4}");
    assert_eq!(silent, 0, "silent errors");
    assert_eq!(review_truth_auto, 0, "review-truth questions auto-accepted");
    assert!(ar <= 0.03, "AR {ar}");

    // ---- Outputs exist.
    for f in [
        "results.csv",
        "results.xlsx",
        "item_analysis.csv",
        "test_stats.csv",
        "item_analysis.xlsx",
        "tp154_a1.csv",
        "tp154_b1.csv",
        "tp154.xlsx",
        "report.html",
        "review_queue.json",
        "booklets.json",
        "summary.json",
        "key.csv",
        "manifest.json",
    ] {
        assert!(out.join(f).is_file(), "{f} missing");
    }
    assert!(out.join("project.omrproj/project.omr").is_file());

    // ---- Review every open item with the truth (audited events + incremental rescoring).
    let open: Vec<(u64, ItemKind, Option<usize>, Option<u16>)> = p
        .queue(None)
        .iter()
        .map(|i| (i.id, i.kind, i.booklet, i.question))
        .collect();
    for (id, kind, b, q) in open {
        let d = match kind {
            ItemKind::Mark => {
                let bk = &p.session.booklets[b.unwrap()];
                decision_of(&final_truth(
                    &truth[&bk.serial].questions[usize::from(q.unwrap())],
                ))
            }
            ItemKind::Identity => {
                let bk = &p.session.booklets[b.unwrap()];
                Decision::Student(truth[&bk.serial].student_id.clone().unwrap())
            }
            ItemKind::Form => {
                let bk = &p.session.booklets[b.unwrap()];
                Decision::Form((truth[&bk.serial].form - 1) as u8)
            }
            ItemKind::Notice => Decision::Acknowledge,
        };
        // Overriding the engine's proposal takes a reason (§13.4): the item family's first.
        let reason = p.overrides_engine(id, &d).then(|| {
            let lane = p.session.review.item(id).unwrap().lane;
            omr_app::review::ReviewReason::code(omr_app::review::reasons_for(kind, lane)[0])
        });
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
    assert!(p.queue(None).is_empty());
    assert_eq!(p.session.release_blocked(), None);
    p.export(&out).unwrap();

    // ---- Scores against an independent hand computation (A8).
    let k = keys();
    let csv = String::from_utf8(read(&out.join("results.csv"))[3..].to_vec()).unwrap();
    let rows: Vec<Vec<String>> = csv
        .lines()
        .map(|l| {
            l.split(',')
                .map(|c| c.trim_matches('"').to_string())
                .collect()
        })
        .collect();
    assert_eq!(rows.len(), 1 + SHEETS as usize);
    let col = |name: &str| rows[0].iter().position(|h| h == name).unwrap();
    let (c_id, c_bk, c_raw, c_pct, c_letter, c_status) = (
        col("Student ID"),
        col("Booklet"),
        col("Raw score"),
        col("Percent"),
        col("Letter"),
        col("Status"),
    );
    for row in &rows[1..] {
        let serial: u32 = row[c_bk].parse().unwrap();
        let t = &truth[&serial];
        let f = usize::from(t.form) - 1;
        let correct = t
            .questions
            .iter()
            .filter(|q| final_truth(q) == Response::Single(k[f][usize::from(q.question)]))
            .count();
        let pct = correct as f64 * 100.0 / f64::from(QUESTIONS);
        let letter = match pct {
            x if x >= 95.0 => "A+",
            x if x >= 90.0 => "A",
            x if x >= 85.0 => "B+",
            x if x >= 80.0 => "B",
            x if x >= 75.0 => "C+",
            x if x >= 70.0 => "C",
            x if x >= 65.0 => "D+",
            x if x >= 60.0 => "D",
            _ => "F",
        };
        assert_eq!(row[c_status], "scored");
        assert_eq!(row[c_id], t.student_id.clone().unwrap());
        assert_eq!(
            row[c_raw].parse::<f64>().unwrap(),
            correct as f64,
            "booklet {serial}"
        );
        assert_eq!(row[c_pct], format!("{pct:.2}"), "booklet {serial}");
        assert_eq!(row[c_letter], letter, "booklet {serial}");
    }

    // ---- A key change is a new key version, rescored without re-projecting (L2).
    let run_before = p
        .scoring
        .as_ref()
        .unwrap()
        .run
        .as_ref()
        .unwrap()
        .run_id
        .clone();
    let k0 = k[0][0];
    p.set_key(0, 1, &char::from(b'A' + (k0 + 1) % OPTIONS).to_string())
        .unwrap();
    let run_after = p
        .scoring
        .as_ref()
        .unwrap()
        .run
        .as_ref()
        .unwrap()
        .run_id
        .clone();
    assert_ne!(run_before, run_after);
    assert_eq!(p.session.key.as_ref().unwrap().version, 2);
    p.set_key(0, 1, &char::from(b'A' + k0).to_string()).unwrap();
    assert_eq!(p.session.key.as_ref().unwrap().version, 3);
    // Mapping edits recompute outcomes on the next query.
    let before = p.outcomes().unwrap().result.clos[0].stats.n;
    p.set_mapping_weight(1, 1, Some(0.5)).unwrap();
    assert!(p.outcomes().is_some());
    assert_eq!(p.outcomes().unwrap().result.clos[0].stats.n, before);
    assert!(p.set_mapping_weight(1, 1, Some(1.5)).is_err());

    // ---- Undo re-opens the latest item and restores the response.
    let n_open = p.queue(None).len();
    let reopened = p.undo().unwrap().unwrap();
    assert_eq!(p.queue(None).len(), n_open + 1);
    assert!(p.session.review.item(reopened).unwrap().is_open());

    // ---- The store holds the audited chain.
    let st = p.store().unwrap();
    st.verify().unwrap();
    assert!(st.event_count() > 100);
    p.close().unwrap();
}

/// Exports that cite wall-clock state and are therefore excluded from the byte comparison:
/// the XLSX workbooks and the HTML report carry the store's event-chain head in their
/// document metadata (the chain hashes event times), `summary.json` holds the run's
/// timings, and `manifest.json` lists the SHA-384 of every file above. Every other export,
/// the decision files, must be byte-identical.
const A5_EXCLUDED: [&str; 6] = [
    "results.xlsx",
    "item_analysis.xlsx",
    "tp154.xlsx",
    "report.html",
    "summary.json",
    "manifest.json",
];

/// Every exported file name in `dir`, sorted.
fn exported(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn req_a5_outputs_identical_across_threads_and_input_order() {
    let dir = tempfile::tempdir().unwrap();
    let (images, _) = corpus(&dir.path().join("corpus"));
    let def = definition();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&images)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    // A seeded shuffle of the page files (one page per sheet here), and the reverse order.
    let mut shuffled = files.clone();
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(5);
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, rng.random_range(0..=i));
    }
    assert_ne!(shuffled, files);
    let mut reversed = files.clone();
    reversed.reverse();
    let runs: [(&str, usize, &[PathBuf]); 5] = [
        ("t1", 1, &files),
        ("t2", 2, &files),
        ("t4", 4, &files),
        ("t4-shuffled", 4, &shuffled),
        ("t2-reversed", 2, &reversed),
    ];
    // Each run's open items are resolved the same way (the engine's proposal, else blank, not
    // on the roster, or the first form) and the results released, so the review decisions
    // and the Blackboard upload are compared too.
    let graded = |inputs: &[PathBuf], out: &Path, threads: usize| {
        let mut p = grade(&def, inputs, out, threads);
        while let Some((id, kind, sugg)) = p
            .queue(None)
            .first()
            .map(|i| (i.id, i.kind, i.suggestion.clone()))
        {
            let decision = match (kind, sugg) {
                (omr_app::ItemKind::Notice, _) => omr_app::Decision::Acknowledge,
                (_, Some(s)) => s,
                (omr_app::ItemKind::Mark, None) => omr_app::Decision::Blank,
                (omr_app::ItemKind::Identity, None) => omr_app::Decision::NotOnRoster,
                (omr_app::ItemKind::Form, None) => omr_app::Decision::Form(0),
            };
            p.apply(
                id,
                omr_app::Action::Resolve {
                    decision,
                    accepted_suggestion: true,
                    note: String::new(),
                },
            )
            .unwrap();
        }
        p.release(None).unwrap();
        p.export(out).unwrap();
        p
    };
    let base_dir = dir.path().join(runs[0].0);
    let base = graded(runs[0].2, &base_dir, runs[0].1);
    let names = exported(&base_dir);
    let mut compared: Vec<String> = Vec::new();
    let mut differing: std::collections::BTreeSet<String> = Default::default();
    for (name, threads, inputs) in &runs[1..] {
        let out = dir.path().join(name);
        let p = graded(inputs, &out, *threads);
        assert_eq!(exported(&out), names, "{name}: another set of files");
        for f in &names {
            let same = read(&base_dir.join(f)) == read(&out.join(f));
            if A5_EXCLUDED.contains(&f.as_str()) {
                if !same {
                    differing.insert(f.clone());
                }
                continue;
            }
            assert!(
                same,
                "{name}: {f} differs from the 1-thread run in name order"
            );
            if !compared.contains(f) {
                compared.push(f.clone());
            }
        }
        // Decisions (every question of every booklet, identities, forms) and the queue.
        assert_eq!(base.session.booklets, p.session.booklets, "{name}");
        assert_eq!(base.session.pages.len(), p.session.pages.len(), "{name}");
        for (x, y) in base.session.pages.iter().zip(&p.session.pages) {
            assert_eq!(
                (&x.page_id, x.status, &x.read),
                (&y.page_id, y.status, &y.read),
                "{name}"
            );
        }
        assert_eq!(
            base.session.review.items.len(),
            p.session.review.items.len(),
            "{name}"
        );
        for (x, y) in base
            .session
            .review
            .items
            .iter()
            .zip(&p.session.review.items)
        {
            assert_eq!(
                (
                    x.code.as_str(),
                    &x.sub_reason,
                    x.severity,
                    x.booklet,
                    x.question,
                    x.group_key,
                    &x.suggestion
                ),
                (
                    y.code.as_str(),
                    &y.sub_reason,
                    y.severity,
                    y.booklet,
                    y.question,
                    y.group_key,
                    &y.suggestion
                ),
                "{name}"
            );
        }
    }
    for must in [
        "results.csv",
        "item_analysis.csv",
        "test_stats.csv",
        "tp154_a1.csv",
        "tp154_b1.csv",
        "booklets.json",
        "review_queue.json",
        "key.csv",
        "blackboard_grade_center.csv",
    ] {
        assert!(
            compared.iter().any(|f| f == must),
            "{must} was not compared"
        );
    }
    eprintln!(
        "A5: {} runs (1, 2, 4 threads; shuffled and reversed order) byte-identical in {} files \
         ({}); excluded and differing: {differing:?}",
        runs.len(),
        compared.len(),
        compared.join(", ")
    );
    if let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("a5-ownform-determinism.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "sheets": SHEETS,
                "runs": runs.iter().map(|(n, t, _)| serde_json::json!({"run": n, "threads": t})).collect::<Vec<_>>(),
                "byte_identical": compared,
                "excluded": A5_EXCLUDED,
                "excluded_and_differing": differing,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn req_a4_rescoring_is_incremental_and_fast() {
    // A synthetic 10,000 × 100 session without images: measure the review-action and
    // key-change rescoring paths (A4 budget 50 ms, release build).
    use omr_app::model::{
        Booklet, EffectiveKey, FormSource, IdentityState, KeyCell, QuestionState, Session,
    };
    use omr_app::score::Scoring;
    let n_q = 100u16;
    let mut def = ExamDefinition::new(
        "A4",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        n_q,
        5,
        vec!["A".into()],
        CancelMode::EraseOnly,
    );
    def.key = KeySource::Inferred;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(4);
    let booklets: Vec<Booklet> = (0..10_000u32)
        .map(|i| Booklet {
            booklet_id: format!("bk-{i:06}"),
            serial: i + 1,
            pages: vec![],
            declared_form: Some(0),
            declared_text: "A".into(),
            form: Some(0),
            form_source: FormSource::Bubbled,
            inference: None,
            hold: None,
            questions: (0..n_q)
                .map(|q| {
                    let r = Response::Single(rng.random_range(0..5u8));
                    QuestionState {
                        recognized: omr_model::QuestionOutcome {
                            question: q,
                            response: r.clone(),
                            auto_accepted: true,
                            logit: omr_model::LogitMilli(9000),
                            exceptions: vec![],
                        },
                        current: r,
                        status: ResponseStatus::Auto,
                        page: 0,
                    }
                })
                .collect(),
            identity: IdentityState {
                read: String::new(),
                student: Some(format!("9{i:08}")),
                status: omr_identity::IdentityStatus::NeedsConfirmation,
                candidates: vec![],
                exceptions: vec![],
                confirmed: false,
            },
            exceptions: vec![],
            sheet_section: String::new(),
            exam_type: String::new(),
        })
        .collect();
    let key = EffectiveKey {
        version: 1,
        inferred: false,
        forms: vec![
            (0..n_q)
                .map(|q| KeyCell::Options(vec![(q % 5) as u8]))
                .collect(),
        ],
        points: vec!["1".into(); usize::from(n_q)],
        shared_items: true,
    };
    let mut session = Session {
        schema: 1,
        definition: def.clone(),
        snapshot_utc: "2026-10-16T09:00:00Z".into(),
        manifest: Default::default(),
        sources: vec![],
        pages: vec![],
        booklets,
        key: Some(key.clone()),
        batch_exceptions: vec![],
        exam_exceptions: vec![],
        review: Default::default(),
        timings: Default::default(),
        release: None,
    };
    let mut sc = Scoring::build(&session).unwrap().unwrap();
    let before = sc.run.as_ref().unwrap().run_id.clone();
    // Review action: one response changed in one booklet.
    let mut best = f64::MAX;
    for k in 0..5 {
        session.booklets[123].questions[7].current = Response::Single(k);
        session.booklets[123].questions[7].status = ResponseStatus::Changed;
        let t = Instant::now();
        sc.update_booklet(123, &session.booklets[123]).unwrap();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    let after = sc.run.as_ref().unwrap().run_id.clone();
    assert_ne!(before, after);
    // The incremental path equals a full rebuild.
    let full = Scoring::build(&session).unwrap().unwrap();
    assert_eq!(full.prep.l1_hash, sc.prep.l1_hash);
    assert_eq!(full.run.as_ref().unwrap(), sc.run.as_ref().unwrap());
    // Key change: five different revisions of one entry, best of five (each is a real change:
    // a new key version and a rescored run).
    let mut key_ms = f64::MAX;
    let mut last_run = sc.run.as_ref().unwrap().run_id.clone();
    for k in 0..5u8 {
        let mut k2 = key.clone();
        k2.forms[0][3] = KeyCell::Options(vec![k % 5, (k + 1) % 5]);
        k2.version = 2 + u32::from(k);
        let t = Instant::now();
        sc.set_key(&def, &k2).unwrap();
        key_ms = key_ms.min(t.elapsed().as_secs_f64() * 1e3);
        let run = sc.run.as_ref().unwrap().run_id.clone();
        assert_ne!(run, last_run, "a key change is a new score run");
        last_run = run;
    }
    // CR-14: a review action in the application also rewrites and binds the session snapshot
    // (serialised, SHA-384, synced, `SnapshotSaved`): only the header and the changed booklet
    // chunk, so it stays near the rescoring cost. Measured end to end at the same size and
    // reported beside the rescoring figure the A4 budget applies to.
    let pkg = tempfile::tempdir().unwrap();
    let st = omr_app::store::StoreWriter::create(
        &pkg.path().join("exam.omrproj"),
        &def,
        "2026-10-16T09:00:00Z",
        "principal:test",
    )
    .unwrap();
    let mut p = ExamProject::from_graded(omr_app::pipeline::Graded {
        session,
        scoring: Some(sc),
        store: Some(st),
    });
    p.save().unwrap();
    // The whole snapshot (header, booklet chunks and their index, pages).
    let snapshot_bytes: u64 = std::fs::read_dir(pkg.path().join("exam.omrproj").join("cache"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    let mut action_ms = f64::MAX;
    for k in 0..5 {
        let t = Instant::now();
        p.session.booklets[321].questions[9].current = Response::Single(k);
        p.session.booklets[321].questions[9].status = ResponseStatus::Changed;
        let b = p.session.booklets[321].clone();
        p.scoring.as_mut().unwrap().update_booklet(321, &b).unwrap();
        p.touch(321);
        p.save().unwrap();
        action_ms = action_ms.min(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!(
        "A4: review-action rescoring {best:.1} ms, key-change rescoring {key_ms:.1} ms (10,000 × 100, best of 5); with the snapshot save {action_ms:.0} ms ({:.1} MB snapshot)",
        snapshot_bytes as f64 / 1e6
    );
    if let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("a4-app-rescoring.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "booklets": 10_000, "items": n_q, "best_of": 5,
                "review_action_rescore_ms": best, "key_change_rescore_ms": key_ms,
                "review_action_with_snapshot_save_ms": action_ms,
                "snapshot_bytes": snapshot_bytes,
                "budget_ms": 50, "release_build": !cfg!(debug_assertions),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    if cfg!(debug_assertions) {
        // Unoptimised builds only report; the budget is asserted in release.
        assert!(best < 2_000.0 && key_ms < 2_000.0);
    } else {
        assert!(best <= 50.0, "A4 review action {best:.1} ms > 50 ms");
        assert!(key_ms <= 50.0, "A4 key change {key_ms:.1} ms > 50 ms");
        // The whole snapshot is never rewritten per action (it was 0.9 s at this size).
        assert!(
            action_ms <= 250.0,
            "review action with its save {action_ms:.0} ms"
        );
    }
}

#[test]
fn req_a10_corrupt_inputs_inferred_key_and_view_models() {
    let dir = tempfile::tempdir().unwrap();
    let corpus_dir = dir.path().join("corpus");
    let t = Template::build(spec()).unwrap();
    let mut cfg = CorpusConfig::standard(8, 77);
    cfg.roster = roster_ids();
    cfg.threads = 4;
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, &corpus_dir).unwrap();
    let images = corpus_dir.join("images");
    // A corrupt JPEG, a truncated PDF and a byte-identical duplicate join the batch.
    let first = std::fs::read_dir(&images)
        .unwrap()
        .map(|e| e.unwrap().path())
        .min()
        .unwrap();
    let extra = dir.path().join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(
        extra.join("broken.jpg"),
        b"\xff\xd8\xff\xe0 not really a jpeg",
    )
    .unwrap();
    std::fs::write(
        extra.join("truncated.pdf"),
        b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog",
    )
    .unwrap();
    std::fs::copy(&first, extra.join("again.jpg")).unwrap();
    let mut def = definition();
    def.key = KeySource::Inferred;
    def.outcomes = None;
    let out = dir.path().join("out");
    let p = grade(&def, &[images.clone(), extra.clone()], &out, 3);
    let s = &p.session;
    use omr_app::model::PageStatus;
    let count = |st: PageStatus| s.pages.iter().filter(|p| p.status == st).count();
    assert_eq!(count(PageStatus::Read), 8);
    assert_eq!(
        count(PageStatus::Refused),
        2,
        "CAP-001 for the corrupt files"
    );
    assert_eq!(count(PageStatus::Duplicate), 1, "CAP-011 for the duplicate");
    for pg in s.pages.iter().filter(|p| p.status == PageStatus::Refused) {
        assert_eq!(pg.refusal.as_ref().unwrap().code, "CAP-001");
    }
    let capture = p.queue(Some(omr_app::Lane::Capture));
    assert_eq!(
        capture.iter().filter(|i| i.code == "CAP-001").count(),
        2,
        "refused pages queue in the Capture lane"
    );
    // Inferred key: SCO-003 `inferred_key` blocks release; every output is marked.
    assert!(s.key.as_ref().unwrap().inferred);
    assert!(
        p.queue(Some(omr_app::Lane::Scoring))
            .iter()
            .any(|i| i.code == "SCO-003" && i.sub_reason.as_deref() == Some("inferred_key"))
    );
    assert!(s.release_blocked().unwrap().contains("INFERRED"));
    let names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "manifest.json" && n != "project.omrproj")
        .collect();
    assert!(!names.is_empty());
    for n in &names {
        assert!(n.contains("-INFERRED"), "{n} not marked");
    }
    let csv = String::from_utf8(read(&out.join("results-INFERRED.csv"))).unwrap();
    assert!(csv.contains("KEY INFERRED"));
    let html = String::from_utf8(read(&out.join("report-INFERRED.html"))).unwrap();
    assert!(html.contains("KEY INFERRED"));
    // The inferred key is each form's modal confident response (checked against the truth:
    // the simulated students answer at random, so this is the mode, not the real key).
    let truth: Vec<PageTruth> =
        omr_sim::corpus::read_truth(&corpus_dir.join("truth.jsonl")).unwrap();
    let ek = s.key.as_ref().unwrap();
    for f in 0..2usize {
        for q in 0..usize::from(QUESTIONS) {
            let mut counts = [0u32; OPTIONS as usize];
            for t in truth.iter().filter(|t| usize::from(t.form) == f + 1) {
                if let Expected::Auto(Response::Single(o)) = &t.questions[q].expected {
                    counts[usize::from(*o)] += 1;
                }
            }
            let best = *counts.iter().max().unwrap();
            match &ek.forms[f][q] {
                omr_app::model::KeyCell::Options(v) => {
                    assert_eq!(v.len(), 1);
                    assert_eq!(counts[usize::from(v[0])], best, "form {f} q{q}");
                }
                omr_app::model::KeyCell::Void => assert_eq!(best, 0, "form {f} q{q}"),
                other => panic!("{other:?}"),
            }
        }
    }

    // ---- View-models for the shell.
    let sum = p.summary_vm();
    assert_eq!(sum.sheets_scanned, 8);
    // CR-18: a batch-scope INFO note counts as INFO, never as a warning that needs a look.
    {
        let before = sum.open_exceptions;
        let mut q = omr_app::ExamProject::from_graded(omr_app::pipeline::Graded {
            session: p.session.clone(),
            scoring: None,
            store: None,
        });
        q.session.batch_exceptions.push(
            omr_model::Exception::must(
                "MRK-016",
                omr_model::Severity::Info,
                omr_model::Scope::Batch,
            )
            .with_sub_reason("prior_model"),
        );
        let after = q.summary_vm().open_exceptions;
        assert_eq!((after.info, after.warn), (before.info + 1, before.warn));
    }
    assert!(sum.key_status.contains("INFERRED"));
    let setup = p.setup_vm();
    assert_eq!(setup.keys.len(), 2);
    assert!(!setup.key_published);
    let q = p.review_queue_vm(omr_app::Lane::Capture);
    assert!(q.groups.iter().any(|g| g.code == "CAP-001"));
    assert!(!q.audit_head.is_empty());
    let res = p.results_vm().unwrap();
    assert_eq!(res.students, 8);
    assert!(res.release_blocked.is_some());
    assert_eq!(res.grades.iter().map(|g| g.count).sum::<u32>(), 8);
    // A mark item renders its crop (RGBA from the stored original) with bubble overlays.
    let mut tracker = omr_app::vm::ProgressTracker::default();
    tracker.push(&omr_app::Progress::Started {
        pages: 2,
        workers: 2,
    });
    let snap = tracker.snapshot();
    assert_eq!((snap.pages_total, snap.workers), (2, 2));
    if let Some(item) = p.queue(Some(omr_app::Lane::Marks)).first().map(|i| i.id) {
        let v = p.review_item_vm(item).unwrap();
        assert_eq!(
            v.crop.rgba.len(),
            (v.crop.width * v.crop.height * 4) as usize
        );
        assert!(v.crop.width > 20 && !v.bubbles.is_empty());
        assert!(v.suggestion.is_some() || v.code == "CAP-009");
    }
    let notice = p
        .queue(Some(omr_app::Lane::Scoring))
        .first()
        .map(|i| i.id)
        .unwrap();
    assert!(p.review_item_vm(notice).unwrap().notice.is_some());
    // The dashboard lists graded exams below a workspace folder (read-only, no store lock).
    let exams = omr_app::vm::list_exams(dir.path());
    assert_eq!(exams.len(), 1);
    assert_eq!(exams[0].1.sheets_scanned, 8);
}

/// A one-page PDF whose page is a single full-page gray image (raw or DCT), placed on A4 so
/// that the effective resolution is `dpi`.
fn pdf_with_image(width: usize, height: usize, data: &[u8], dct: bool, dpi: f64) -> Vec<u8> {
    let (w_pt, h_pt) = (width as f64 * 72.0 / dpi, height as f64 * 72.0 / dpi);
    let content = format!("q {w_pt:.4} 0 0 {h_pt:.4} 0 0 cm /Im0 Do Q");
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    obj(
        &mut out,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w_pt:.4} {h_pt:.4}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .as_bytes(),
    );
    obj(
        &mut out,
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        )
        .as_bytes(),
    );
    let mut img = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceGray /BitsPerComponent 8 {}/Length {} >>\nstream\n",
        if dct { "/Filter /DCTDecode " } else { "" },
        data.len()
    )
    .into_bytes();
    img.extend_from_slice(data);
    img.extend_from_slice(b"\nendstream");
    obj(&mut out, &img);
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn own_form_sheets_at_300_dpi_as_jpeg_and_in_pdfs_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let corpus_dir = dir.path().join("corpus");
    let t = Template::build(spec()).unwrap();
    let mut cfg = CorpusConfig::standard(3, 300);
    cfg.dpi = 300;
    cfg.roster = roster_ids();
    cfg.threads = 3;
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, &corpus_dir).unwrap();
    let truth: BTreeMap<u32, PageTruth> =
        omr_sim::corpus::read_truth(&corpus_dir.join("truth.jsonl"))
            .unwrap()
            .into_iter()
            .map(|p| (p.booklet, p))
            .collect();
    let images = corpus_dir.join("images");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&images)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let check = |p: &ExamProject, expect: usize| {
        let s = &p.session;
        assert_eq!(
            s.booklets.len(),
            expect,
            "{:?}",
            s.pages
                .iter()
                .map(|p| (p.status, p.refusal.clone()))
                .collect::<Vec<_>>()
        );
        for b in &s.booklets {
            let t = &truth[&b.serial];
            for (q, qs) in b.questions.iter().enumerate() {
                if qs.status == ResponseStatus::Auto
                    && let Expected::Auto(r) = &t.questions[q].expected
                {
                    assert_eq!(*r, qs.current, "booklet {} q{q}", b.serial);
                }
            }
            let auto = b
                .questions
                .iter()
                .filter(|q| q.status == ResponseStatus::Auto)
                .count();
            assert!(auto >= usize::from(QUESTIONS) - 3, "auto {auto}");
            assert_eq!(b.identity.student.as_deref(), t.student_id.as_deref());
        }
        for pg in &s.pages {
            // A4 at 200 dpi is 1654 px wide; the simulated scan adds a margin.
            let w = pg.working.unwrap().0;
            assert!((1600..1800).contains(&w), "resampled to 200 dpi: {w}");
            assert_eq!(pg.source_dpi, Some(300));
        }
    };
    // 300-dpi JPEG files.
    let def = definition();
    let p = grade(&def, &files, &dir.path().join("jpeg"), 2);
    check(&p, 3);
    // The same sheets as 300-dpi PDFs: one raw gray image, one DCT (JPEG) image.
    let d0 = omr_imaging::decode(&std::fs::read(&files[0]).unwrap())
        .unwrap()
        .image;
    let raw = pdf_with_image(d0.width(), d0.height(), d0.as_raw(), false, 300.0);
    let jpeg = std::fs::read(&files[1]).unwrap();
    let d1 = omr_imaging::decode(&jpeg).unwrap().image;
    let dct = pdf_with_image(d1.width(), d1.height(), &jpeg, true, 300.0);
    let pdfs = dir.path().join("pdfs");
    std::fs::create_dir_all(&pdfs).unwrap();
    std::fs::write(pdfs.join("raw.pdf"), raw).unwrap();
    std::fs::write(pdfs.join("dct.pdf"), dct).unwrap();
    let p = grade(
        &def,
        std::slice::from_ref(&pdfs),
        &dir.path().join("pdf"),
        2,
    );
    check(&p, 2);
}
