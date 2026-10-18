//! The `omr` binary on the real-world cases (synthetic sheets only, `omr-sim` own forms): a
//! later batch added to a graded exam (C4), the real key after `--infer-key` and a mis-key
//! correction (C5), a roster with a duplicate ID (C6), override reasons (C7) and the
//! Blackboard upload (C8); every refusal exits with its documented code and says what to do.

use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: [u8; 32] = [0x44; 32];

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

fn fails(o: &Output, code: i32) -> String {
    assert_eq!(
        o.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_4444_2222_7333_8444_0000_0000_0007,
        )),
        20,
        4,
        CancelMode::EraseOnly,
    );
    s.forms = 1;
    s
}

/// Eight simulated sheets split into two days, the spec, key and roster files.
fn fixture(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let t = Template::build(spec()).unwrap();
    let mut cfg = omr_sim::CorpusConfig::standard(8, 23);
    let ids: Vec<String> = (0..8).map(|i| format!("41000{:04}", i * 5 + 2)).collect();
    cfg.roster = ids.clone();
    cfg.threads = 2;
    // Ticks and crosses instead of fills: review items with a proposal (for C7).
    cfg.rates.check_or_x = 0.08;
    let corpus = dir.join("corpus");
    omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, &corpus).unwrap();
    let (day1, day2) = (dir.join("day1"), dir.join("day2"));
    std::fs::create_dir_all(&day1).unwrap();
    std::fs::create_dir_all(&day2).unwrap();
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus.join("images"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    for (k, f) in files.iter().enumerate() {
        let to = if k < 4 { &day1 } else { &day2 };
        std::fs::copy(f, to.join(f.file_name().unwrap())).unwrap();
    }
    let specf = dir.join("spec.json");
    std::fs::write(
        &specf,
        serde_json::to_vec(&json!({"template": spec(), "exam_key_hex": hex::encode(KEY)})).unwrap(),
    )
    .unwrap();
    // The key as an instructor saves it from Excel: BOM, CRLF, lowercase, no header.
    let mut key = String::from("\u{feff}");
    for q in 1..=20 {
        key.push_str(&format!("{q},{}\r\n", ["a", "b", "c", "d"][q % 4]));
    }
    std::fs::write(dir.join("key.csv"), key).unwrap();
    let mut roster = String::from("\u{feff}student_id,name,الاسم العربي\r\n");
    for (i, id) in ids.iter().enumerate() {
        roster.push_str(&format!("{id},Test {i},طالب {i}\r\n"));
    }
    std::fs::write(dir.join("roster.csv"), &roster).unwrap();
    // The same roster with one ID twice.
    roster.push_str(&format!("{},Someone Else,آخر\r\n", ids[3]));
    std::fs::write(dir.join("roster-dup.csv"), roster).unwrap();
    (day1, day2, specf)
}

#[test]
fn omr_cli_real_world_cases() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (day1, day2, specf) = fixture(d);
    let outd = d.join("out");
    let grade = |extra: &[&str], out: &Path| {
        let mut a = vec![
            "grade",
            p(&day1),
            "--spec",
            p(&specf),
            "--questions",
            "20",
            "--id-scheme",
            "digits:9",
            "--out",
            p(out),
            "--threads",
            "2",
            "--snapshot-time",
            "2026-10-17T08:00:00Z",
            "--quiet",
        ];
        a.extend_from_slice(extra);
        omr(&a)
    };
    // C6: a roster with a duplicate ID is refused before grading, saying which rows.
    let e = fails(
        &grade(
            &["--infer-key", "--roster", p(&d.join("roster-dup.csv"))],
            &d.join("dup"),
        ),
        3,
    );
    assert!(e.contains("appears 2 times") && e.contains("remove"), "{e}");
    // Day 1 with an inferred key.
    let g = ok(&grade(
        &["--infer-key", "--roster", p(&d.join("roster.csv"))],
        &outd,
    ));
    assert!(g.contains("4 page(s) → 4 booklet(s)"), "{g}");
    assert!(outd.join("results-INFERRED.csv").exists());
    // C4: day 2 added; the exam has eight booklets.
    let a = ok(&omr(&[
        "add",
        p(&day2),
        "--project",
        p(&outd),
        "--threads",
        "2",
    ]));
    assert!(a.contains("batch-2") && a.contains("booklets 4 → 8"), "{a}");
    // A file of day 1 again: CAP-011, nothing added.
    let again = d.join("again");
    std::fs::create_dir_all(&again).unwrap();
    let f1 = std::fs::read_dir(&day1)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::copy(&f1, again.join("copy.jpg")).unwrap();
    let a = ok(&omr(&["add", p(&again), "--project", p(&outd)]));
    assert!(
        a.contains("1 already imported") && a.contains("booklets 8 → 8"),
        "{a}"
    );
    // C5: the real key replaces the inferred one; outputs are no longer INFERRED.
    let k = ok(&omr(&[
        "key",
        "replace",
        "--project",
        p(&outd),
        "--key",
        p(&d.join("key.csv")),
    ]));
    let version = |s: &str| -> u32 {
        s.split("key version ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    // Day 2 re-inferred the key (a new version); the supplied key is the next one.
    let v = version(&k);
    assert!(v >= 2, "{k}");
    assert!(outd.join("results.csv").exists());
    // A mis-key corrected with its reason; a malformed answer says what to write.
    let k = ok(&omr(&[
        "key",
        "set",
        "--project",
        p(&outd),
        "--question",
        "3",
        "--answer",
        "a",
        "--reason",
        "KEY_ERROR",
    ]));
    assert_eq!(version(&k), v + 1, "{k}");
    let e = fails(
        &omr(&[
            "key",
            "set",
            "--project",
            p(&outd),
            "--question",
            "3",
            "--answer",
            "Z",
        ]),
        6,
    );
    assert!(e.contains("one letter"), "{e}");
    // C7: overriding the engine's proposal needs --reason.
    let list: Value = serde_json::from_str(&ok(&omr(&[
        "review",
        "list",
        "--project",
        p(&outd),
        "--json",
    ])))
    .unwrap();
    let item = list["lanes"]["Marks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["suggestion"].as_str().is_some_and(|s| s.len() == 1));
    let item = item.expect("a mark item with a proposal");
    {
        let id = item["id"].as_u64().unwrap().to_string();
        let other = if item["suggestion"] == "A" { "B" } else { "A" };
        let resolve = |extra: &[&str]| {
            let mut a = vec![
                "review",
                "resolve",
                "--project",
                p(&outd),
                "--item",
                &id,
                "--decision",
                other,
                "--no-export",
            ];
            a.extend_from_slice(extra);
            omr(&a)
        };
        let e = fails(&resolve(&[]), 6);
        assert!(
            e.contains("--reason") && e.contains("AMBIGUOUS_MARK"),
            "{e}"
        );
        let e = fails(&resolve(&["--reason", "OTHER"]), 6);
        assert!(e.contains("needs a note"), "{e}");
        ok(&resolve(&[
            "--reason",
            "OTHER",
            "--note",
            "checked the paper sheet",
        ]));
    }
    // CR-07: release is the instructor's act, refused while items are open (exit 6, with the
    // next command); every open item resolved with the engine's proposal, then released.
    let e = fails(&omr(&["release", "--project", p(&outd)]), 6);
    assert!(
        e.contains("release refused") && e.contains("omr review list"),
        "{e}"
    );
    loop {
        let list: Value = serde_json::from_str(&ok(&omr(&[
            "review",
            "list",
            "--project",
            p(&outd),
            "--json",
            "--reveal",
        ])))
        .unwrap();
        let Some(it) = list["lanes"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|v| v.as_array().unwrap().clone())
            .next()
        else {
            break;
        };
        let sugg = it["suggestion"].as_str().unwrap_or("").to_string();
        let decision = match (it["kind"].as_str().unwrap(), sugg.as_str()) {
            ("Notice", _) => "ack".to_string(),
            ("Mark", "") => "blank".to_string(),
            ("Identity", "") => "not-on-roster".to_string(),
            ("Form", "") => "form:A".to_string(),
            (_, s) if s.starts_with("multi ") => s.replace("multi ", "multi:"),
            (_, s) if s.starts_with("student ") => s.replace("student ", "student:"),
            (_, s) if s.starts_with("form ") => s.replace("form ", "form:"),
            (_, "not on roster") => "not-on-roster".to_string(),
            (_, s) => s.to_string(),
        };
        let id = it["id"].as_u64().unwrap().to_string();
        ok(&omr(&[
            "review",
            "resolve",
            "--project",
            p(&outd),
            "--item",
            &id,
            "--decision",
            &decision,
            "--no-export",
        ]));
    }
    let r = ok(&omr(&["release", "--project", p(&outd)]));
    assert!(
        r.contains("released") && r.contains("Blackboard upload"),
        "{r}"
    );
    assert!(outd.join("blackboard_grade_center.csv").exists());
    // C8: the Blackboard upload, also for an existing grade column.
    let x = ok(&omr(&[
        "export",
        "--project",
        p(&outd),
        "--blackboard-column",
        "835932",
    ]));
    assert!(x.contains("column 835932"), "{x}");
    let bb =
        std::fs::read_to_string(outd.join("blackboard_grade_center-column-835932.csv")).unwrap();
    assert!(
        bb.contains("\"Username\",\"Exam [Total Pts: 20 Score] |835932\""),
        "{bb}"
    );
    assert!(!bb.contains("Test ") && !bb.contains("طالب"));
    let e = fails(
        &omr(&[
            "export",
            "--project",
            p(&outd),
            "--blackboard-column",
            "83/5",
        ]),
        3,
    );
    assert!(e.contains("Grade Center download"), "{e}");
}
