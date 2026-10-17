//! The `omr` binary end to end on generated sheets (no real data): templates, sheet rendering,
//! `grade` with a key, roster and CLO file, `review list|resolve` (+ undo), `bench`, and the
//! documented exit codes for bad input.

use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: [u8; 32] = [0x33; 32];

fn omr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_omr"))
        .args(args)
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

fn spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_1111_2222_7333_8444_0000_0000_0005,
        )),
        30,
        4,
        CancelMode::EraseOnly,
    );
    s.forms = 1;
    s
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// 6 simulated sheets + spec, key, roster and CLO files.
fn fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let t = Template::build(spec()).unwrap();
    let mut cfg = omr_sim::CorpusConfig::standard(6, 5);
    let ids: Vec<String> = (0..6).map(|i| format!("31000{:04}", i * 3 + 1)).collect();
    cfg.roster = ids.clone();
    cfg.threads = 2;
    let corpus = dir.join("corpus");
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, &corpus).unwrap();
    let specf = dir.join("spec.json");
    std::fs::write(
        &specf,
        serde_json::to_vec(&json!({"template": spec(), "exam_key_hex": hex::encode(KEY)})).unwrap(),
    )
    .unwrap();
    let mut key = String::from("question,A\n");
    for q in 1..=30 {
        key.push_str(&format!("{q},{}\n", ["A", "B", "C", "D"][q % 4]));
    }
    std::fs::write(dir.join("key.csv"), key).unwrap();
    let mut roster = String::from("student_id,name,section\n");
    for (i, id) in ids.iter().enumerate() {
        roster.push_str(&format!("{id},Test {i},10{}\n", i % 2));
    }
    std::fs::write(dir.join("roster.csv"), roster).unwrap();
    std::fs::write(
        dir.join("clo.csv"),
        "clo,domain,title,plo,questions,weight\nCLO1,K,Know,K1,1-15,1\nCLO2,S,Do,S1,16-30,1\n",
    )
    .unwrap();
    // A corrupt scan joins the batch: CAP-001 on that page, the batch continues.
    std::fs::write(
        corpus.join("images").join("zz-broken.jpg"),
        b"\xff\xd8 broken",
    )
    .unwrap();
    (corpus.join("images"), specf)
}

#[test]
fn omr_cli_grades_reviews_and_benches() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let out = ok(&omr(&["templates"]));
    assert!(out.contains("iau-ccsit-v1"));
    let ex: Value = serde_json::from_str(&ok(&omr(&["templates", "--example-spec"]))).unwrap();
    assert!(ex["template"]["questions"].is_number());
    let (images, specf) = fixture(d);

    // Sheet rendering.
    let pdf = d.join("sheets.pdf");
    ok(&omr(&[
        "sheet",
        "render",
        "--spec",
        p(&specf),
        "--out",
        p(&pdf),
        "--copies",
        "2",
    ]));
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));

    // Grading.
    let outd = d.join("out");
    let g = ok(&omr(&[
        "grade",
        p(&images),
        "--spec",
        p(&specf),
        "--questions",
        "30",
        "--key",
        p(&d.join("key.csv")),
        "--roster",
        p(&d.join("roster.csv")),
        "--clo",
        p(&d.join("clo.csv")),
        "--id-scheme",
        "digits:9",
        "--out",
        p(&outd),
        "--threads",
        "2",
        "--snapshot-time",
        "2026-10-16T09:00:00Z",
        "--quiet",
    ]));
    assert!(g.contains("7 page(s) → 6 booklet(s)"), "{g}");
    for f in [
        "results.csv",
        "results.xlsx",
        "item_analysis.csv",
        "tp154_a1.csv",
        "tp154_b1.csv",
        "report.html",
        "review_queue.json",
        "manifest.json",
    ] {
        assert!(outd.join(f).is_file(), "{f}");
    }
    // Grading into the same folder again is refused (exit 3).
    let again = omr(&[
        "grade",
        p(&images),
        "--spec",
        p(&specf),
        "--questions",
        "30",
        "--out",
        p(&outd),
    ]);
    assert_eq!(again.status.code(), Some(3));

    // Review.
    let list: Value = serde_json::from_str(&ok(&omr(&[
        "review",
        "list",
        "--project",
        p(&outd),
        "--json",
    ])))
    .unwrap();
    let items: Vec<Value> = list["lanes"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|v| v.as_array().unwrap().clone())
        .collect();
    let open_before = list["open"].as_u64().unwrap();
    let cap = items
        .iter()
        .find(|i| i["code"] == "CAP-001")
        .expect("the corrupt scan is a Capture item");
    assert_eq!(cap["lane"], "Capture");
    let first = items.iter().find(|i| i["kind"] == "Mark").unwrap_or(cap);
    {
        let id = first["id"].as_u64().unwrap().to_string();
        let decision = if first["kind"] == "Mark" {
            let s = first["suggestion"].as_str().unwrap_or("blank").to_string();
            if s.starts_with("multi") {
                s.replace("multi ", "multi:")
            } else {
                s
            }
        } else {
            "ack".to_string()
        };
        ok(&omr(&[
            "review",
            "resolve",
            "--project",
            p(&outd),
            "--item",
            &id,
            "--decision",
            &decision,
        ]));
        let after: Value = serde_json::from_str(&ok(&omr(&[
            "review",
            "list",
            "--project",
            p(&outd),
            "--json",
        ])))
        .unwrap();
        assert_eq!(after["open"].as_u64().unwrap(), open_before - 1);
        ok(&omr(&[
            "review",
            "resolve",
            "--project",
            p(&outd),
            "--undo",
        ]));
        let undone: Value = serde_json::from_str(&ok(&omr(&[
            "review",
            "list",
            "--project",
            p(&outd),
            "--json",
        ])))
        .unwrap();
        assert_eq!(undone["open"].as_u64().unwrap(), open_before);
        // A decision the item cannot take is a review error (exit 6).
        let bad = omr(&[
            "review",
            "resolve",
            "--project",
            p(&outd),
            "--item",
            &id,
            "--decision",
            "student:123",
            "--no-export",
        ]);
        assert_eq!(
            bad.status.code(),
            Some(6),
            "{}",
            String::from_utf8_lossy(&bad.stderr)
        );
    }
    let txt = ok(&omr(&["review", "list", "--project", p(&outd)]));
    assert!(txt.contains("open item(s)"));

    // Benchmark (small).
    let bench_json = d.join("bench.json");
    ok(&omr(&[
        "bench",
        p(&images),
        "--spec",
        p(&specf),
        "--questions",
        "30",
        "--threads",
        "1,2",
        "--first",
        "3",
        "--scratch",
        p(&d.join("scratch")),
        "--out",
        p(&bench_json),
    ]));
    let b: Value = serde_json::from_slice(&std::fs::read(&bench_json).unwrap()).unwrap();
    assert_eq!(b["runs"].as_array().unwrap().len(), 2);
    assert!(b["ttfr"]["seconds"].as_f64().unwrap() > 0.0);
}

#[test]
fn omr_cli_exit_codes_for_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let out = d.join("o");
    // Unknown template: definition error (2).
    let o = omr(&[
        "grade",
        p(d),
        "--template",
        "nope",
        "--questions",
        "5",
        "--out",
        p(&out),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown legacy template"));
    // Missing input: input error (3).
    let o = omr(&[
        "grade",
        p(&d.join("missing.pdf")),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "29",
        "--out",
        p(&out),
    ]);
    assert_eq!(o.status.code(), Some(3));
    // Too many questions for the sheet (2).
    let o = omr(&[
        "grade",
        p(d),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "120",
        "--out",
        p(&out),
    ]);
    assert_eq!(o.status.code(), Some(2));
    // Review of a folder without a graded exam: not found (7).
    let o = omr(&["review", "list", "--project", p(d)]);
    assert_eq!(o.status.code(), Some(7));
    // A bad snapshot time is refused before any work (2), leaving no project behind.
    let o = omr(&[
        "grade",
        p(d),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "29",
        "--snapshot-time",
        "17/10/2026",
        "--out",
        p(&out),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(!out.join("project.omrproj").exists());
    // A form the legacy sheet cannot bubble (2); an ID scheme of 0 digits (2).
    for extra in [["--forms", "A,E"], ["--id-scheme", "digits:0"]] {
        let mut args = vec![
            "grade",
            p(d),
            "--template",
            "iau-ccsit-v1",
            "--questions",
            "29",
            "--out",
            p(&out),
        ];
        args.extend(extra);
        assert_eq!(omr(&args).status.code(), Some(2), "{extra:?}");
    }
    // clap usage errors exit 2, also for review flags that exclude each other.
    assert_eq!(omr(&["grade"]).status.code(), Some(2));
    assert_eq!(
        omr(&[
            "review",
            "resolve",
            "--project",
            p(d),
            "--undo",
            "--item",
            "1"
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        omr(&[
            "review",
            "resolve",
            "--project",
            p(d),
            "--item",
            "1",
            "--defer",
            "--decision",
            "A"
        ])
        .status
        .code(),
        Some(2)
    );
}
