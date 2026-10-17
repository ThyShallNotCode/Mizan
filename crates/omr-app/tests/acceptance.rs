//! Acceptance rows proven at the application level (docs/IMPLEMENTATION.md §1), where the
//! engine-level tests stop: A5 on the legacy IAU path (the path the real sheets take), A7 for
//! forged and foreign sheets reaching the review queue, and A10 for cut-off answers that must
//! stay pending through scoring. Synthetic data only: the committed blank IAU form with
//! synthetic pencil marks, and `omr-sim` own-form sheets.

mod common;

use common::{Marks, PdfImage};
use omr_app::model::{PageStatus, ResponseStatus};
use omr_app::review::Lane;
use omr_app::{ExamDefinition, ExamProject, FormKey, KeySource, TemplateChoice};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, Response, Severity};
use rand::{RngExt, SeedableRng};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

fn answers(k: u16, n: u16) -> Vec<Option<u8>> {
    let v = k % 2;
    (0..n)
        .map(|q| {
            if (q + k) % 11 == 7 {
                None
            } else if (q + 2 * k).is_multiple_of(9) {
                Some((key(v, q) + 1) % 5)
            } else {
                Some(key(v, q))
            }
        })
        .collect()
}

fn student(k: u16) -> String {
    format!("22103457{k:02}")
}

fn legacy_def(n: u16) -> ExamDefinition {
    common::legacy_def(
        "Acceptance",
        n,
        &["A", "B"],
        KeySource::Provided {
            forms: (0..2u16)
                .map(|v| FormKey {
                    form: ["A", "B"][usize::from(v)].into(),
                    answers: (0..n)
                        .map(|q| char::from(b'A' + key(v, q)).to_string())
                        .collect(),
                })
                .collect(),
        },
    )
}

/// Exports that cite wall-clock state (see `synthetic_e2e.rs`, `A5_EXCLUDED`): the XLSX
/// workbooks and the HTML report carry the event-chain head, `summary.json` the timings,
/// `manifest.json` the hashes of all of them.
const A5_EXCLUDED: [&str; 6] = [
    "results.xlsx",
    "item_analysis.xlsx",
    "tp154.xlsx",
    "report.html",
    "summary.json",
    "manifest.json",
];

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

/// A5 on the legacy path: 12 IAU sheets as 200-dpi PNG files plus a 600-dpi bitonal PDF of 6
/// more (two batch slices), graded at 1, 2 and 4 threads in name order and at 4 threads in a
/// seeded shuffle of the files: every decision file byte-identical, the excluded ones listed.
#[test]
fn req_a5_legacy_decisions_identical_at_1_2_4_threads_and_shuffled_order() {
    let dir = tempfile::tempdir().unwrap();
    let e = common::iau();
    let scans = dir.path().join("scans");
    let mut files = Vec::new();
    for k in 0..12u16 {
        let m = Marks::new(&answers(k, 29), &student(k), Some((k % 2) as u8));
        let img = common::sheet(&e, &m, u64::from(k) + 1);
        files.push(common::write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &common::png(&img),
        ));
    }
    let pdf_pages: Vec<PdfImage> = (12..18u16)
        .map(|k| {
            let m = Marks::new(&answers(k, 29), &student(k), Some((k % 2) as u8));
            let img = common::rescale(&common::sheet(&e, &m, u64::from(k) + 1), 3, 1);
            PdfImage::bitonal(&img, 600.0)
        })
        .collect();
    files.push(common::write(&scans, "batch.pdf", &common::pdf(&pdf_pages)));
    files.sort();
    let mut shuffled = files.clone();
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(55);
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, rng.random_range(0..=i));
    }
    assert_ne!(shuffled, files);
    let def = legacy_def(29);
    let runs: [(&str, usize, &[PathBuf]); 4] = [
        ("t1", 1, &files),
        ("t2", 2, &files),
        ("t4", 4, &files),
        ("t4-shuffled", 4, &shuffled),
    ];
    let base_dir = dir.path().join(runs[0].0);
    let base = common::grade(&def, runs[0].2, &base_dir, runs[0].1);
    assert_eq!(base.session.booklets.len(), 18);
    let names = exported(&base_dir);
    let mut compared = BTreeSet::new();
    for (name, threads, inputs) in &runs[1..] {
        let out = dir.path().join(name);
        let p = common::grade(&def, inputs, &out, *threads);
        assert_eq!(exported(&out), names, "{name}");
        for f in names.iter().filter(|f| !A5_EXCLUDED.contains(&f.as_str())) {
            assert!(
                std::fs::read(base_dir.join(f)).unwrap() == std::fs::read(out.join(f)).unwrap(),
                "{name}: {f} differs from the 1-thread run in name order"
            );
            compared.insert(f.clone());
        }
        assert_eq!(base.session.booklets, p.session.booklets, "{name}");
        let pages = |p: &ExamProject| -> Vec<_> {
            p.session
                .pages
                .iter()
                .map(|x| (x.page_id.clone(), x.status, x.read.clone()))
                .collect()
        };
        assert_eq!(pages(&base), pages(&p), "{name}");
    }
    for must in [
        "results.csv",
        "booklets.json",
        "review_queue.json",
        "key.csv",
        "item_analysis.csv",
    ] {
        assert!(compared.contains(must), "{must} not compared");
    }
    if let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("a5-legacy-determinism.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "sheets": 18, "slices": ["200-dpi gray PNG files", "600-dpi bitonal PDF"],
                "runs": runs.iter().map(|(n, t, _)| serde_json::json!({"run": n, "threads": t})).collect::<Vec<_>>(),
                "byte_identical": compared, "excluded": A5_EXCLUDED,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

const OWN_KEY: [u8; 32] = [0x7a; 32];

fn own_spec(exam: u128, questions: u16) -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(exam)),
        questions,
        4,
        CancelMode::EraseOnly,
    );
    s.forms = 1;
    s
}

/// A7 at the application level: genuine sheets of the exam, one sheet printed with the
/// exam's layout but another page-code key (forged), and one sheet of another exam (foreign)
/// in one batch. The forged and the foreign page are never attached to a booklet: each is held
/// unattributed with IDN-002 (BLOCKER) in the Capture lane, release is blocked, and the
/// genuine booklets read normally.
#[test]
fn req_a7_forged_and_foreign_sheets_are_held_with_idn002() {
    let dir = tempfile::tempdir().unwrap();
    const EXAM: u128 = 0x0190_a7a7_0000_7000_8000_0000_0000_0007;
    let spec = own_spec(EXAM, 20);
    let t = Template::build(spec.clone()).unwrap();
    let write = |sub: &str, t: &Template, key: [u8; 32], sheets: u32, seed: u64| {
        let mut cfg = omr_sim::CorpusConfig::standard(sheets, seed);
        cfg.threads = 2;
        let d = dir.path().join(sub);
        omr_sim::corpus::write_corpus(t, &ExamKey::from_bytes(key), &cfg, &d).unwrap();
        d.join("images")
    };
    let genuine = write("genuine", &t, OWN_KEY, 3, 71);
    let forged = write("forged", &t, [0x13; 32], 1, 72);
    let other = Template::build(own_spec(0x0190_0bad_0000_7000_8000_0000_0000_0099, 20)).unwrap();
    let foreign = write("foreign", &other, [0x29; 32], 1, 73);
    let mut def = ExamDefinition::new(
        "A7",
        TemplateChoice::Generated(Box::new(spec)),
        20,
        4,
        vec!["A".into()],
        CancelMode::EraseOnly,
    );
    def.page_code_key_hex = Some(hex::encode(OWN_KEY));
    def.key = KeySource::Provided {
        forms: vec![FormKey {
            form: "A".into(),
            answers: (0..20)
                .map(|q| ["A", "B", "C", "D"][q % 4].into())
                .collect(),
        }],
    };
    def.exam_id = def.derived_exam_id();
    def.validate().unwrap();
    let out = dir.path().join("out");
    let p = common::grade(&def, &[genuine, forged, foreign], &out, 2);
    let s = &p.session;
    assert_eq!(s.pages.len(), 5);
    assert_eq!(s.booklets.len(), 3, "only the genuine sheets are booklets");
    let held: Vec<usize> = (0..s.pages.len())
        .filter(|&i| s.pages[i].status != PageStatus::Read)
        .collect();
    assert_eq!(
        held.len(),
        2,
        "{:?}",
        s.pages.iter().map(|x| x.status).collect::<Vec<_>>()
    );
    for &pi in &held {
        assert_eq!(s.pages[pi].status, PageStatus::Unattributed);
        assert!(s.pages[pi].booklet.is_none());
        let items: Vec<_> = p
            .queue(None)
            .into_iter()
            .filter(|i| i.page == Some(pi))
            .map(|i| (i.code.clone(), i.severity, i.lane))
            .collect();
        assert!(
            items.contains(&("IDN-002".into(), Severity::Blocker, Lane::Capture)),
            "page {pi}: {items:?}"
        );
    }
    for b in &s.booklets {
        assert!(b.questions.iter().all(|q| !q.current.is_unreadable()));
    }
    assert!(s.release_blocked().is_some());
}

/// A10 at the application level: `UNREADABLE` is never `EMPTY`. A 100-question IAU sheet
/// scanned with its right edge cut off loses the last answer block: those questions are
/// `Unreadable` with CAP-009, pending in the Marks lane, the booklet's total is provisional,
/// and nothing about them reaches the results as blank or wrong. The other sheets score
/// normally.
#[test]
fn req_a10_cut_off_answers_stay_pending_never_scored_blank() {
    let dir = tempfile::tempdir().unwrap();
    let e = common::iau();
    let scans = dir.path().join("scans");
    let mut inputs = Vec::new();
    for k in 0..3u16 {
        let m = Marks::new(&answers(k, 100), &student(k), Some((k % 2) as u8));
        let mut img = common::sheet(&e, &m, u64::from(k) + 1);
        if k == 1 {
            // Cut at x = 1,900 px (200 dpi): the block of questions 81–100 (x ≥ 2,064) is gone;
            // the height is kept, so the page still reads as 200 dpi.
            img = img.crop(0, 0, 1_900, img.height(), 255);
        }
        inputs.push(common::write(
            &scans,
            &format!("sheet-{k:02}.png"),
            &common::png(&img),
        ));
    }
    let p = common::grade(&legacy_def(100), &inputs, &dir.path().join("out"), 2);
    let s = &p.session;
    assert_eq!(s.booklets.len(), 3);
    let cut = (0..3)
        .find(|&bi| common::source_of(&p, bi) == "sheet-01.png")
        .unwrap();
    let sc = p.scoring.as_ref().unwrap();
    let run = sc.run.as_ref().unwrap();
    let mut unreadable = 0;
    for (q, qs) in s.booklets[cut].questions.iter().enumerate() {
        if q < 80 {
            assert!(!qs.current.is_unreadable(), "q{}", q + 1);
            continue;
        }
        unreadable += 1;
        assert!(
            qs.current.is_unreadable(),
            "q{}: {:?} (a cut-off answer must never read as {:?})",
            q + 1,
            qs.current,
            Response::Blank
        );
        assert_eq!(qs.status, ResponseStatus::Pending, "q{}", q + 1);
        assert!(
            qs.recognized
                .exceptions
                .iter()
                .any(|x| x.code == "CAP-009" && x.severity == Severity::Error),
            "q{}: {:?}",
            q + 1,
            qs.recognized.exceptions
        );
        assert!(
            p.queue(Some(Lane::Marks))
                .iter()
                .any(|i| i.booklet == Some(cut) && i.question == Some(q as u16)),
            "q{} is not queued",
            q + 1
        );
    }
    assert_eq!(unreadable, 20);
    let row = |bi: usize| &run.students[sc.row_of[bi].unwrap()];
    assert_eq!(
        row(cut).status,
        omr_scoring::AssessmentStatus::Provisional,
        "pending answers hold the total"
    );
    for bi in (0..3).filter(|&bi| bi != cut) {
        assert_eq!(row(bi).status, omr_scoring::AssessmentStatus::Scored);
    }
    assert!(s.release_blocked().is_some());
}
