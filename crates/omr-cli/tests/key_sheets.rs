//! The `omr` binary with teacher-filled key sheets (spec §10.1, CR-11), synthetic sheets only
//! (the committed blank IAU form with synthetic pencil marks, a synthetic key):
//! - `omr key read` writes `key.csv` (the `--key` layout) and `key_summary.html`; a row that is
//!   not one confidently read option is left empty and the command exits 5 naming it;
//! - `omr grade --key-sheet` holds the run (nothing scored, release refused, `review list`
//!   names the hold) until `omr key confirm`, which publishes the key and scores the exam;
//! - `--answer` decides the rows the key sheet leaves open.

#[path = "../../omr-app/tests/common/mod.rs"]
mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

thread_local! {
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

fn fails(o: &Output, code: i32) -> (String, String) {
    assert_eq!(
        o.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

const N: u16 = 20;

fn key(v: u16, q: u16) -> u8 {
    ((q * 2 + v * 3) % 5) as u8
}

fn letter(o: u8) -> char {
    char::from(b'A' + o)
}

fn key_sheet(dir: &Path, name: &str, v: u16, marker: char, seed: u64, multi_q3: bool) -> PathBuf {
    let e = iau();
    let mut answers: Vec<Vec<u8>> = (0..N).map(|q| vec![key(v, q)]).collect();
    if multi_q3 {
        answers[2].push((key(v, 2) + 1) % 5);
    }
    let m = Marks {
        answers,
        id: std::iter::repeat_n(vec![marker as u8 - b'0'], 10).collect(),
        version: Some(v as u8),
        ..Marks::default()
    };
    write(dir, name, &png(&sheet(&e, &m, seed)))
}

fn students(dir: &Path) -> Vec<PathBuf> {
    let e = iau();
    (0..4u16)
        .map(|k| {
            let v = k % 2;
            let answers: Vec<Option<u8>> = (0..N)
                .map(|q| {
                    if q.is_multiple_of(k + 2) {
                        Some((key(v, q) + 1) % 5)
                    } else {
                        Some(key(v, q))
                    }
                })
                .collect();
            let m = Marks::new(&answers, &format!("22104600{k:02}"), Some(v as u8));
            write(
                dir,
                &format!("s{k}.png"),
                &png(&sheet(&e, &m, 60 + u64::from(k))),
            )
        })
        .collect()
}

fn expected_csv() -> String {
    let mut s = String::from("question,A,B,points\r\n");
    for q in 0..N {
        s.push_str(&format!(
            "{},{},{},1\r\n",
            q + 1,
            letter(key(0, q)),
            letter(key(1, q))
        ));
    }
    s
}

#[test]
fn key_read_writes_the_key_file_and_its_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("keys");
    let a = key_sheet(&dir, "key-A.png", 0, '0', 3, false);
    let b = key_sheet(&dir, "key-B.png", 1, '9', 4, false);
    let out = tmp.path().join("read");
    let o = ok(&omr(&[
        "key",
        "read",
        p(&a),
        p(&b),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "20",
        "--out",
        p(&out),
    ]));
    assert!(
        o.contains("version A: 1 key sheet(s), 20 questions, 0 row(s) need your decision"),
        "{o}"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("key.csv")).unwrap(),
        expected_csv()
    );
    let html = std::fs::read_to_string(out.join("key_summary.html")).unwrap();
    assert!(html.contains("Version A") && html.contains("Version B"));
    // The key file is usable as it is.
    let def = omr_app::exam::read_key_file(
        &std::fs::read(out.join("key.csv")).unwrap(),
        &["A".into(), "B".into()],
        N,
        5,
    )
    .unwrap();
    assert_eq!(def.0.len(), 2);
    // A multi-mark row: left empty, exit 5 naming the question; a sheet without a marker is
    // refused (exit 5), never a key.
    let m = key_sheet(&dir, "key-multi.png", 0, '0', 5, true);
    let none = key_sheet(&dir, "key-shifted.png", 0, '1', 6, false);
    let out2 = tmp.path().join("read2");
    let (stdout, stderr) = fails(
        &omr(&[
            "key",
            "read",
            p(&m),
            p(&none),
            "--template",
            "iau-ccsit-v1",
            "--questions",
            "20",
            "--forms",
            "A",
            "--out",
            p(&out2),
        ]),
        5,
    );
    assert!(
        stderr.contains("questions 3 need your decision"),
        "{stderr}"
    );
    assert!(
        stdout.contains("NOT USED") && stdout.contains("1111111111"),
        "{stdout}"
    );
    let csv = std::fs::read_to_string(out2.join("key.csv")).unwrap();
    assert_eq!(csv.lines().nth(3), Some("3,,1"));
}

#[test]
fn grade_with_key_sheets_is_held_until_the_instructor_confirms() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = tmp.path().join("scans");
    let st = students(&scans);
    let keys = tmp.path().join("keys");
    let a = key_sheet(&keys, "key-A.png", 0, '0', 13, true);
    let b = key_sheet(&keys, "key-B.png", 1, '9', 14, false);
    let out = tmp.path().join("exam");
    let mut args = vec!["grade"];
    args.extend(st.iter().map(|s| p(s)));
    args.extend([
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "20",
        "--key-sheet",
        p(&a),
        "--key-sheet",
        p(&b),
        "--out",
        p(&out),
        "--snapshot-time",
        "2026-10-17T12:00:00Z",
        "--quiet",
    ]);
    let o = ok(&omr(&args));
    assert!(
        o.contains("key sheets: 2 (2 named, 0 found in the stack)"),
        "{o}"
    );
    assert!(
        o.contains(
            "version A: 20 questions, NOT CONFIRMED; 1 row(s) need your decision (questions 3)"
        ),
        "{o}"
    );
    assert!(o.contains("omr key confirm"), "{o}");
    assert!(out.join("key_sheet_summary.html").exists());
    assert!(
        !out.join("results.csv").exists(),
        "nothing scored before confirmation"
    );
    // Release refused; the review list names the hold.
    let (_, e) = fails(&omr(&["release", "--project", p(&out)]), 6);
    assert!(e.contains("SCO-003"), "{e}");
    let l = ok(&omr(&["review", "list", "--project", p(&out)]));
    assert!(l.contains("HOLD SCO-003 BLOCKER"), "{l}");
    // Confirming without deciding question 3 is refused, naming it.
    let (_, e) = fails(&omr(&["key", "confirm", "--project", p(&out)]), 6);
    assert!(e.contains("version A question 3"), "{e}");
    // A key entry cannot be changed around the unconfirmed key.
    fails(
        &omr(&[
            "key",
            "set",
            "--project",
            p(&out),
            "--question",
            "3",
            "--answer",
            "B",
        ]),
        6,
    );
    let q3 = format!("A:3={}", letter(key(0, 2)));
    let c = ok(&omr(&[
        "key",
        "confirm",
        "--project",
        p(&out),
        "--answer",
        &q3,
        "--operator",
        "principal:dr-a",
    ]));
    assert!(c.contains("confirmed the key of version(s) A, B"), "{c}");
    assert!(c.contains("key version 1 published"), "{c}");
    assert_eq!(
        std::fs::read_to_string(out.join("key.csv")).unwrap(),
        expected_csv()
    );
    assert!(out.join("results.csv").exists());
    let again = fails(&omr(&["key", "confirm", "--project", p(&out)]), 6).1;
    assert!(again.contains("already confirmed"), "{again}");
}

/// C7: key sheets read after grading take the audited path. `omr key read` says it is only a
/// preview; `omr key replace --from-key-sheets` adds the sheets to the graded exam (their
/// originals stored with the scans), holds it, and `omr key confirm` publishes the key with the
/// key sheets as its source.
#[test]
fn key_sheets_added_to_a_graded_exam_are_stored_held_and_confirmed() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = tmp.path().join("scans");
    let st = students(&scans);
    let keys = tmp.path().join("keys");
    let a = key_sheet(&keys, "key-A.png", 0, '0', 13, false);
    let b = key_sheet(&keys, "key-B.png", 1, '9', 14, false);
    let preview = ok(&omr(&[
        "key",
        "read",
        p(&a),
        p(&b),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "20",
        "--out",
        p(&tmp.path().join("preview")),
    ]));
    assert!(
        preview.contains("not an audited confirmation") && preview.contains("--from-key-sheets"),
        "{preview}"
    );
    let out = tmp.path().join("exam");
    let mut args = vec!["grade"];
    args.extend(st.iter().map(|s| p(s)));
    args.extend([
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "20",
        "--infer-key",
        "--out",
        p(&out),
        "--snapshot-time",
        "2026-10-17T12:00:00Z",
        "--quiet",
    ]);
    ok(&omr(&args));
    let r = ok(&omr(&[
        "key",
        "replace",
        "--project",
        p(&out),
        "--from-key-sheets",
        p(&a),
        p(&b),
    ]));
    assert!(r.contains("key-sheet file(s) added"), "{r}");
    assert!(
        r.contains("key sheets: 2 (2 named, 0 found in the stack)"),
        "{r}"
    );
    assert!(r.contains("omr key confirm"), "{r}");
    let (_, e) = fails(&omr(&["release", "--project", p(&out)]), 6);
    assert!(e.contains("SCO-003"), "{e}");
    ok(&omr(&["key", "confirm", "--project", p(&out)]));
    // The key in force is the key sheets' (every row, as the synthetic key).
    let key_csv = std::fs::read_to_string(out.join("key.csv")).unwrap();
    for line in expected_csv().lines().skip(1) {
        assert!(key_csv.contains(line), "{line} in {key_csv}");
    }
    // The project records the key sheets as the key's source, with their scans.
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["key"], "key sheets, confirmed");
    assert_eq!(summary["key_sheets"], 2);
    // Neither file is a student any more.
    assert_eq!(summary["booklets"], 4);
}
