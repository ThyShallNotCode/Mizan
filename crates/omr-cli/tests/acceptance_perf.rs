//! Requirements A2 (throughput) and A3 (time to first result) through the `omr` binary
//! (docs/IMPLEMENTATION.md §1; spec §2.4, §21.1), on two synthetic corpora written to disk:
//! - **own forms**: `omr-sim` sheets (100 questions × 5 options, two forms, 9-digit ID grid),
//!   standard capture profile, 200-dpi gray JPEG at the sampled quality (70–95);
//! - **legacy IAU**: the committed blank IAU CCSIT form with synthetic pencil marks, delivered
//!   as the university's scanners deliver it: 600-dpi bitonal PDFs of 30 pages (1-bit Flate,
//!   where the real Canon PDFs use CCITT G4).
//!
//! The tests are `#[ignore]`d: they time the machine, so `scripts/acceptance.sh` runs them in
//! release and one at a time (`--include-ignored --test-threads=1`) and collects the JSON they
//! write to `$OMR_ACCEPT_OUT`. Synthetic data only.

#[path = "../../omr-app/tests/common/mod.rs"]
mod common;

use common::{Marks, PdfImage};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const OWN_KEY: [u8; 32] = [0x5e; 32];
const OWN_QUESTIONS: u16 = 100;
const LEGACY_QUESTIONS: u16 = 29;
/// A2 floor: the spec's 4-core figure (§2.4, §21.1); the stretch is 100.
const A2_FLOOR_PAGES_PER_S: f64 = 40.0;
/// A3 budget: 30 sheets end to end.
const A3_BUDGET_S: f64 = 3.0;

thread_local! {
    /// This test's private key folder (device key, external checkpoints, exam registry), so
    /// tests that grade the same sheets in parallel never see each other's projects.
    static KEY_DIR: tempfile::TempDir = tempfile::tempdir().unwrap();
}

fn omr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_omr"))
        .args(args)
        .env("OMR_KEY_DIR", KEY_DIR.with(|d| d.path().to_path_buf()))
        .output()
        .expect("run omr")
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Writes `v` to `$OMR_ACCEPT_OUT/<name>.json` when the acceptance script sets it.
fn report(name: &str, v: &Value) {
    let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        Path::new(&dir).join(format!("{name}.json")),
        serde_json::to_vec_pretty(v).unwrap(),
    )
    .unwrap();
}

/// A synthetic corpus on disk and the files `omr grade` needs.
struct Corpus {
    /// Folder of scans (JPEG files or PDFs).
    scans: PathBuf,
    /// `--template NAME` or `--spec FILE`.
    exam: Vec<String>,
    questions: u16,
    key: PathBuf,
    roster: PathBuf,
    clo: PathBuf,
    id_scheme: &'static str,
    pages: usize,
}

impl Corpus {
    fn exam_args(&self) -> Vec<String> {
        let mut v = self.exam.clone();
        v.extend([
            "--questions".into(),
            self.questions.to_string(),
            "--forms".into(),
            "A,B".into(),
            "--id-scheme".into(),
            self.id_scheme.into(),
        ]);
        v
    }
}

fn own_spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_a2a3_0000_7000_8000_0000_0000_0001,
        )),
        OWN_QUESTIONS,
        5,
        CancelMode::EraseOnly,
    );
    s.forms = 2;
    s.title = "Acceptance A2/A3".into();
    s.course = "SIM 101".into();
    s
}

/// `sheets` own-form sheets (one page each) written with `omr-sim` below `dir`.
fn own_corpus(dir: &Path, sheets: u32) -> Corpus {
    let t = Template::build(own_spec()).unwrap();
    let mut cfg = omr_sim::CorpusConfig::standard(sheets, 20_261_017);
    cfg.roster = (0..400)
        .map(|i| format!("44{:07}", 1_000 + i * 37))
        .collect();
    cfg.threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let s = omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(OWN_KEY), &cfg, dir).unwrap();
    let spec = dir.join("spec.json");
    std::fs::write(
        &spec,
        serde_json::to_vec(&json!({"template": own_spec(), "exam_key_hex": hex::encode(OWN_KEY)}))
            .unwrap(),
    )
    .unwrap();
    let mut key = String::from("question,A,B\n");
    for q in 0..usize::from(OWN_QUESTIONS) {
        let l = |f: usize| char::from(b'A' + ((q * 7 + f * 3) % 5) as u8);
        key.push_str(&format!("{},{},{}\n", q + 1, l(0), l(1)));
    }
    let mut roster = String::from("student_id,name,section\n");
    for (i, id) in cfg.roster.iter().enumerate() {
        roster.push_str(&format!("{id},Student {i},10{}\n", i % 3));
    }
    let files = |name: &str, text: String| {
        let f = dir.join(name);
        std::fs::write(&f, text).unwrap();
        f
    };
    Corpus {
        scans: dir.join("images"),
        exam: vec!["--spec".into(), p(&spec).into()],
        questions: OWN_QUESTIONS,
        key: files("key.csv", key),
        roster: files("roster.csv", roster),
        clo: files(
            "clo.csv",
            "clo,domain,title,plo,questions,weight\nCLO1,K,Know,K1,1-50,1\nCLO2,S,Do,S1,51-100,1\n"
                .into(),
        ),
        id_scheme: "digits:9",
        pages: s.pages as usize,
    }
}

fn legacy_key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

/// What legacy sheet `k` marks: mostly the key of its version, some wrong, a few blank.
fn legacy_marks(k: u16) -> Marks {
    let v = k % 2;
    let answers: Vec<Option<u8>> = (0..LEGACY_QUESTIONS)
        .map(|q| {
            if (q + k) % 13 == 5 {
                None
            } else if (q + 2 * k).is_multiple_of(7) {
                Some((legacy_key(v, q) + 1) % 5)
            } else {
                Some(legacy_key(v, q))
            }
        })
        .collect();
    Marks::new(
        &answers,
        &format!("2210{:06}", 3_000 + u32::from(k)),
        Some(v as u8),
    )
}

/// `sheets` legacy IAU sheets as 600-dpi bitonal PDFs of 30 pages each below `dir`.
fn legacy_corpus(dir: &Path, sheets: u16) -> Corpus {
    let e = common::iau();
    let scans = dir.join("pdfs");
    std::fs::create_dir_all(&scans).unwrap();
    let next = AtomicUsize::new(0);
    let pages: Mutex<Vec<(u16, PdfImage)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..std::thread::available_parallelism().map_or(1, |n| n.get()) {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed) as u16;
                    if k >= sheets {
                        break;
                    }
                    let page200 = common::sheet(&e, &legacy_marks(k), u64::from(k) + 1);
                    let page600 = common::rescale(&page200, 3, 1);
                    let pdf = PdfImage::bitonal(&page600, 600.0);
                    pages.lock().unwrap().push((k, pdf));
                }
            });
        }
    });
    let mut pages = pages.into_inner().unwrap();
    pages.sort_by_key(|(k, _)| *k);
    let images: Vec<PdfImage> = pages.into_iter().map(|(_, i)| i).collect();
    for (b, chunk) in images.chunks(30).enumerate() {
        common::write(
            &scans,
            &format!("batch-{:02}.pdf", b + 1),
            &common::pdf(chunk),
        );
    }
    let mut key = String::from("question,A,B\n");
    for q in 0..LEGACY_QUESTIONS {
        let l = |v: u16| char::from(b'A' + legacy_key(v, q));
        key.push_str(&format!("{},{},{}\n", q + 1, l(0), l(1)));
    }
    let mut roster = String::from("student_id,name,section\n");
    for k in 0..sheets {
        roster.push_str(&format!(
            "2210{:06},Student {k},10{}\n",
            3_000 + u32::from(k),
            k % 2
        ));
    }
    let files = |name: &str, text: String| {
        let f = dir.join(name);
        std::fs::write(&f, text).unwrap();
        f
    };
    Corpus {
        scans,
        exam: vec!["--template".into(), "iau-ccsit-v1".into()],
        questions: LEGACY_QUESTIONS,
        key: files("key.csv", key),
        roster: files("roster.csv", roster),
        clo: files(
            "clo.csv",
            "clo,domain,title,plo,questions,weight\nCLO1,K,Know,K1,1-15,1\nCLO2,S,Do,S1,16-29,1\n"
                .into(),
        ),
        id_scheme: "iau",
        pages: usize::from(sheets),
    }
}

/// `omr bench` at 1, 2 and 4 threads (best of 3), plus its in-process 30-sheet run.
fn bench(c: &Corpus, work: &Path) -> Value {
    let out = work.join("bench.json");
    let mut args: Vec<String> = vec!["bench".into(), p(&c.scans).into()];
    args.extend(c.exam_args());
    args.extend(
        [
            "--threads",
            "1,2,4",
            "--first",
            "30",
            "--repeat",
            "3",
            "--scratch",
            p(&work.join("scratch")),
            "--out",
            p(&out),
        ]
        .map(String::from),
    );
    let text = ok(&omr(&args.iter().map(String::as_str).collect::<Vec<_>>()));
    eprint!("{text}");
    serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap()
}

fn pages_per_s_at(b: &Value, threads: u64, field: &str) -> f64 {
    b["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["threads"] == threads)
        .and_then(|r| r[field].as_f64())
        .unwrap_or_else(|| panic!("no {threads}-thread run in {b:#}"))
}

fn check_a2(name: &str, c: &Corpus, b: &Value) {
    let rec4 = pages_per_s_at(b, 4, "recognition_pages_per_s");
    let full4 = pages_per_s_at(b, 4, "pipeline_pages_per_s");
    report(
        name,
        &json!({
            "corpus_pages": c.pages, "questions": c.questions, "bench": b,
            "recognition_pages_per_s_4_threads": rec4, "pipeline_pages_per_s_4_threads": full4,
            "floor_pages_per_s": A2_FLOOR_PAGES_PER_S,
        }),
    );
    assert_eq!(b["runs"][0]["pages"].as_u64().unwrap() as usize, c.pages);
    assert!(
        rec4 >= A2_FLOOR_PAGES_PER_S,
        "A2: {rec4:.1} pages/s at 4 threads is below the {A2_FLOOR_PAGES_PER_S} floor (is the \
         machine loaded? rerun scripts/acceptance.sh on a quiet machine before changing code)"
    );
}

#[test]
#[ignore = "timing run: scripts/acceptance.sh runs it in release, one test at a time"]
fn req_a2_throughput_ownform_200dpi_jpeg_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let c = own_corpus(&dir.path().join("corpus"), 240);
    let b = bench(&c, dir.path());
    check_a2("a2-ownform-200dpi-jpeg", &c, &b);
}

#[test]
#[ignore = "timing run: scripts/acceptance.sh runs it in release, one test at a time"]
fn req_a2_throughput_legacy_iau_bitonal_pdf_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let c = legacy_corpus(&dir.path().join("corpus"), 150);
    let b = bench(&c, dir.path());
    check_a2("a2-legacy-600dpi-bitonal-pdf", &c, &b);
}

/// The `omr grade` process on exactly 30 sheets, timed from process start to exit: read,
/// spool (fsync), page and batch stages, identity against the roster, scoring, CLO
/// attainment, item analysis, the store's event chain and every export.
fn grade_30(c: &Corpus, out: &Path) -> f64 {
    let mut args: Vec<String> = vec!["grade".into(), p(&c.scans).into()];
    args.extend(c.exam_args());
    args.extend(
        [
            "--key",
            p(&c.key),
            "--roster",
            p(&c.roster),
            "--clo",
            p(&c.clo),
            "--threads",
            "4",
            "--snapshot-time",
            "2026-10-17T09:00:00Z",
            "--quiet",
            "--out",
            p(out),
        ]
        .map(String::from),
    );
    let t0 = Instant::now();
    let o = omr(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let wall = t0.elapsed().as_secs_f64();
    ok(&o);
    let summary: Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert!(out.join("results.csv").exists() && out.join("manifest.json").exists());
    assert_eq!(summary["pages"]["read"], 30, "{summary:#}");
    eprintln!("A3: {} sheets graded end to end in {wall:.2} s", c.pages);
    wall
}

#[test]
#[ignore = "timing run: scripts/acceptance.sh runs it in release, one test at a time"]
fn req_a3_time_to_first_result_30_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let own = own_corpus(&dir.path().join("own"), 30);
    let legacy = legacy_corpus(&dir.path().join("legacy"), 30);
    assert_eq!((own.pages, legacy.pages), (30, 30));
    let t_own = grade_30(&own, &dir.path().join("own-out"));
    let t_legacy = grade_30(&legacy, &dir.path().join("legacy-out"));
    report(
        "a3-time-to-first-result",
        &json!({
            "sheets": 30, "threads": 4, "budget_s": A3_BUDGET_S,
            "what": "omr grade process wall time: start, read, spool, recognition, identity, scoring, outcomes, analytics, store, exports",
            "ownform_200dpi_jpeg_s": t_own, "legacy_600dpi_bitonal_pdf_s": t_legacy,
        }),
    );
    assert!(t_own < A3_BUDGET_S, "A3 own forms: {t_own:.2} s");
    assert!(t_legacy < A3_BUDGET_S, "A3 legacy: {t_legacy:.2} s");
}
