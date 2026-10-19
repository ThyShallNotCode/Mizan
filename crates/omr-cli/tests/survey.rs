//! `omr survey` (spec G-3 §C, CLI parity): the scans are surveyed and the exam definition is
//! proposed, each value with its source; the command exits 2 while the proposal asks a
//! question and 0 once every question is answered, with `--json` for scripts. Synthetic sheets
//! only (the committed blank IAU form with synthetic pencil marks).

#[path = "../../omr-app/tests/common/mod.rs"]
mod common;

use common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn omr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_omr"))
        .args(args)
        .output()
        .expect("run omr")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The key of version `v` at question `q`.
fn key(v: u8, q: u16) -> u8 {
    ((q * 3) % 5 + u16::from(v) * (q % 2)) as u8 % 5
}

/// Three sheets bubbling A and three bubbling B, 15 rows answered.
fn stack(dir: &Path) -> PathBuf {
    let e = iau();
    let scans = dir.join("scans");
    for k in 0..6u8 {
        let v = k / 3;
        let answers: Vec<Option<u8>> = (0..15).map(|q| Some(key(v, q))).collect();
        let m = Marks::new(&answers, &format!("2210345{k:03}"), Some(v));
        write(
            &scans,
            &format!("s{k}.png"),
            &png(&sheet(&e, &m, 40 + u64::from(k))),
        );
    }
    scans
}

#[test]
fn survey_exits_2_while_a_question_is_open_and_0_once_answered() {
    let dir = tempfile::tempdir().unwrap();
    let scans = stack(dir.path());
    let s = scans.to_str().unwrap();
    let ws = dir.path().to_str().unwrap();
    // No key: the proposal asks for one, never infers it by default.
    let o = omr(&["survey", s, "--workspace", ws, "--quiet"]);
    assert_eq!(o.status.code(), Some(2), "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("question 1 (Answer key): no answer key"),
        "{out}"
    );
    assert!(out.contains("--answer provisional-key"), "{out}");
    assert!(
        stderr(&o).contains("1 question(s) to answer"),
        "{}",
        stderr(&o)
    );
    // The same as JSON: the survey's counts and the proposal's question with its answers.
    let o = omr(&["survey", s, "--workspace", ws, "--quiet", "--json"]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["survey"]["booklets"], 6);
    assert_eq!(v["survey"]["versions"]["by_letter"]["A"], 3);
    assert_eq!(v["proposal"]["ready"], false);
    let issue = v["proposal"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["level"] == "ask")
        .unwrap();
    assert_eq!(issue["field"], "key");
    assert!(
        issue["choices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["cli"] == "--answer provisional-key"),
        "{issue}"
    );
    let questions = v["proposal"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "questions")
        .unwrap()
        .clone();
    assert_eq!(
        (questions["value"].as_str(), questions["source"].as_str()),
        (Some("15"), Some("sheets"))
    );
    // Answered (a provisional key; version B, on too few sheets to infer its key from, held):
    // ready, exit 0, nothing written.
    let before = std::fs::read_dir(dir.path()).unwrap().count();
    let o = omr(&[
        "survey",
        s,
        "--workspace",
        ws,
        "--quiet",
        "--answer",
        "provisional-key",
    ]);
    assert_eq!(o.status.code(), Some(2), "{}{}", stdout(&o), stderr(&o));
    assert!(stdout(&o).contains("--answer hold:B"), "{}", stdout(&o));
    let o = omr(&[
        "survey",
        s,
        "--workspace",
        ws,
        "--quiet",
        "--answer",
        "provisional-key",
        "--answer",
        "hold:B",
    ]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("ready to grade into"), "{out}");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        before,
        "nothing written"
    );
    // A key file names the versions and the count.
    let mut csv = String::from("question,A,B\n");
    for q in 0..15 {
        csv.push_str(&format!(
            "{},{},{}\n",
            q + 1,
            char::from(b'A' + key(0, q)),
            char::from(b'A' + key(1, q))
        ));
    }
    let k = write(dir.path(), "key.csv", csv.as_bytes());
    let o = omr(&[
        "survey",
        s,
        "--key",
        k.to_str().unwrap(),
        "--workspace",
        ws,
        "--json",
        "--quiet",
    ]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["proposal"]["ready"], true);
    let field = |id: &str| {
        v["proposal"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(field("versions")["value"], "A, B");
    assert_eq!(field("versions")["source"], "key-file");
    assert_eq!(field("questions")["value"], "15");
    // Counts only: no student number in what the command prints.
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(!all.contains("2210345"), "{all}");
    // A malformed answer is a usage error.
    let o = omr(&["survey", s, "--answer", "hold", "--quiet"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("--answer hold"), "{}", stderr(&o));
}
