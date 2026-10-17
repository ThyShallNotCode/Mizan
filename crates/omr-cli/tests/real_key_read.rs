//! `omr key read` on the instructors' real key sheets (exam-confidential: nothing is committed
//! or printed but counts). Skips unless `OMR_IAU_PDF_DIR` (holding the key-sheet PDFs) and
//! `OMR_IAU_KEYS` (the verified `key_arti503.csv` and `key_cs522.csv`) are set; `OMR_E2E_OUT`
//! receives the outputs (else a temporary folder).
//!
//! - ARTI 503: two 600-dpi bitonal scans (versions A and B, student number all 0) must read as
//!   exactly the verified key: 56 of 56 answers, nothing to decide.
//! - CS 522: two phone photos (A: all 0, B: all 9). Read correctly they must equal the verified
//!   key (58 of 58); otherwise they must be refused or left to decide: never a wrong answer.
//! - The key summaries highlight a row only when it needs a look: fewer than 5 % of the rows
//!   on each (C2: the photos' unmarked bubbles read 0.10–0.14 and used to flag 48 of 58 rows
//!   as erasure residue).

use std::path::{Path, PathBuf};
use std::process::Command;

fn cells(csv: &Path, questions: u16) -> Vec<(String, Vec<String>)> {
    let (forms, _) = omr_app::exam::read_key_file(
        &std::fs::read(csv).expect("key file"),
        &["A".into(), "B".into()],
        questions,
        5,
    )
    .expect("key file layout");
    forms
        .into_iter()
        .map(|f| {
            let a = f
                .answers
                .iter()
                .map(|a| {
                    let c: omr_app::model::KeyCell =
                        omr_app::exam::parse_key_entry(a, 5).expect("entry").into();
                    c.text()
                })
                .collect();
            (f.form, a)
        })
        .collect()
}

/// (answers equal to the verified key, answers that differ, answers left empty).
fn compare(read: &Path, truth: &Path, questions: u16) -> (usize, usize, usize) {
    let truth = cells(truth, questions);
    let (mut equal, mut wrong, mut empty) = (0, 0, 0);
    // Versions the read key file lacks count as empty.
    let read = if read.exists() {
        let text = std::fs::read_to_string(read).unwrap();
        let header: Vec<&str> = text.lines().next().unwrap_or("").split(',').collect();
        let forms: Vec<String> = header
            .iter()
            .filter(|h| h.len() == 1)
            .map(|h| h.to_string())
            .collect();
        let (got, _) = omr_app::exam::read_key_file(text.as_bytes(), &forms, questions, 5)
            .expect("written key file");
        got.into_iter()
            .map(|f| (f.form, f.answers))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for (form, t) in &truth {
        let r = read.iter().find(|(f, _)| f == form).map(|(_, a)| a);
        for (q, want) in t.iter().enumerate() {
            match r.and_then(|a| a.get(q)).filter(|a| !a.trim().is_empty()) {
                None => empty += 1,
                Some(a) => {
                    let c: omr_app::model::KeyCell =
                        omr_app::exam::parse_key_entry(a, 5).expect("entry").into();
                    if c.text() == *want {
                        equal += 1;
                    } else {
                        wrong += 1;
                    }
                }
            }
        }
    }
    (equal, wrong, empty)
}

#[test]
fn req_a8_real_key_sheets_read_as_the_verified_keys_or_are_refused() {
    let (Ok(dir), Ok(keys)) = (
        std::env::var("OMR_IAU_PDF_DIR"),
        std::env::var("OMR_IAU_KEYS"),
    ) else {
        eprintln!("skipped: OMR_IAU_PDF_DIR / OMR_IAU_KEYS not set");
        return;
    };
    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    pdfs.sort();
    let named = |pat: &str| -> Vec<PathBuf> {
        pdfs.iter()
            .filter(|p| p.to_string_lossy().contains(pat))
            .cloned()
            .collect()
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = std::env::var("OMR_E2E_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    let key_dir = tempfile::tempdir().unwrap();
    let run = |files: &[PathBuf], q: u16, out: &Path| {
        let _ = std::fs::remove_dir_all(out);
        let mut args: Vec<String> = vec!["key".into(), "read".into()];
        args.extend(files.iter().map(|f| f.display().to_string()));
        args.extend(
            [
                "--template",
                "iau-ccsit-v1",
                "--questions",
                &q.to_string(),
                "--out",
                &out.display().to_string(),
            ]
            .map(String::from),
        );
        Command::new(env!("CARGO_BIN_EXE_omr"))
            .args(&args)
            .env("OMR_KEY_DIR", key_dir.path())
            .output()
            .expect("run omr")
            .status
            .code()
    };
    let arti = named("ARTI-503_Final_");
    let cs = named("Key_-_A__B");
    assert_eq!((arti.len(), cs.len()), (2, 1), "the three key-sheet PDFs");
    let out_a = root.join("key-read-arti503");
    let code_a = run(&arti, 28, &out_a);
    let (eq_a, wrong_a, empty_a) = compare(
        &out_a.join("key.csv"),
        &Path::new(&keys).join("key_arti503.csv"),
        28,
    );
    let out_c = root.join("key-read-cs522");
    let code_c = run(&cs, 29, &out_c);
    let (eq_c, wrong_c, empty_c) = compare(
        &out_c.join("key.csv"),
        &Path::new(&keys).join("key_cs522.csv"),
        29,
    );
    eprintln!(
        "arti503: exit {code_a:?}, {eq_a} equal, {wrong_a} wrong, {empty_a} empty; cs522: exit \
         {code_c:?}, {eq_c} equal, {wrong_c} wrong, {empty_c} empty"
    );
    // Highlighted rows of each summary page (counts only).
    let highlighted = |out: &Path| {
        std::fs::read_to_string(out.join("key_summary.html"))
            .map(|h| h.matches("<tr class=\"hi\"").count())
            .unwrap_or(usize::MAX)
    };
    let (hi_a, hi_c) = (highlighted(&out_a), highlighted(&out_c));
    eprintln!("highlighted rows: arti503 {hi_a} of 56, cs522 {hi_c} of 58");
    assert!(hi_a * 20 < 56, "arti503: {hi_a} of 56 rows highlighted");
    assert!(hi_c * 20 < 58, "cs522: {hi_c} of 58 rows highlighted");
    assert_eq!(code_a, Some(0), "arti503 key read");
    assert_eq!((eq_a, wrong_a, empty_a), (56, 0, 0), "arti503 answers");
    assert_eq!(wrong_c, 0, "cs522: a key-sheet answer read wrongly");
    if code_c == Some(0) {
        assert_eq!((eq_c, empty_c), (58, 0), "cs522 answers");
    } else {
        // Refused or left to decide (exit 5): never a wrong key.
        assert_eq!(code_c, Some(5), "cs522 key read");
    }
}
