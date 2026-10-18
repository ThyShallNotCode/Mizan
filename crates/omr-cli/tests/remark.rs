//! `omr import remark` and `omr compare remark` (spec §3.5, §23.7) on synthetic data: Remark
//! workbooks laid out as Remark Office OMR's graded Excel export (invented IDs and names) and
//! IAU sheets rendered on the committed blank form.
//! - the import shows its summary (counts, masked IDs, never a name) and writes `key.csv`,
//!   `clo.csv` and `version_map.csv` owner-only; `--dry-run` writes nothing; points the totals
//!   cannot determine are refused (exit 3) naming the questions, and `--points` settles them;
//!   Remark's binary files are refused;
//! - the exam graded with the imported key, the comparison prints the Restricted banner and
//!   the counts and writes the report where asked.

#[path = "../../omr-app/tests/common/mod.rs"]
mod common;
#[path = "../../omr-app/tests/remark_common/mod.rs"]
mod remark_common;

use remark_common::{Book, Student, letter, xlsx};
use std::path::Path;
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

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

const N: usize = 8;

fn key(v: u8, q: usize) -> u8 {
    ((q as u8) * 2 + 3 * v) % 5
}

/// Student `k` of version `v`: wrong on about 3 questions in 7 (a fixed scatter).
fn answers(k: usize, v: u8) -> Vec<Option<u8>> {
    (0..N)
        .map(|q| {
            let h = (k as u64 * 2_654_435_761 + q as u64 * 40_503 + 12_345)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                >> 40;
            if h % 7 < 3 {
                Some((key(v, q) + 1) % 5)
            } else {
                Some(key(v, q))
            }
        })
        .collect()
}

/// The points of every question, as a points file.
const POINTS: &[u8] =
    b"question,points\n1,0.5\n2,0.75\n3,0.75\n4,0.5\n5,0.75\n6,0.75\n7,0.5\n8,0.75\n";

fn id(k: usize) -> String {
    format!("22200000{k:02}")
}

fn book(v: u8, ks: &[usize], all_right: &[usize]) -> Book {
    let kl: Vec<String> = (0..N).map(|q| letter(key(v, q))).collect();
    let students = ks
        .iter()
        .map(|&k| {
            let mut a: Vec<String> = answers(k, v).iter().map(|o| letter(o.unwrap())).collect();
            for &q in all_right {
                a[q] = kl[q].clone();
            }
            let refs: Vec<&str> = a.iter().map(String::as_str).collect();
            let mut s = Student::new(&id(k), if v == 0 { "A" } else { "B" }, &refs);
            s.name = format!("اسم {k}");
            s
        })
        .collect();
    Book {
        sections: v == 0,
        key: kl,
        points: (0..N)
            .map(|q| if q % 3 == 0 { 0.5 } else { 0.75 })
            .collect(),
        groups: Vec::new(),
        students,
        footer: true,
    }
}

#[test]
fn import_writes_the_key_after_its_summary_and_refuses_undetermined_points() {
    let d = tempfile::tempdir().unwrap();
    let a = common::write(
        d.path(),
        "A_Graded.xlsx",
        &xlsx(&book(0, &(0..16).collect::<Vec<_>>(), &[])),
    );
    let b = common::write(
        d.path(),
        "B_Graded.xlsx",
        &xlsx(&book(1, &(16..32).collect::<Vec<_>>(), &[])),
    );
    // Preview only.
    let out = d.path().join("import");
    let s = ok(&omr(&["import", "remark", p(&a), p(&b), "--dry-run"]));
    assert!(s.contains("2 version(s) (A, B), 8 questions"), "{s}");
    assert!(
        s.contains("Remark totals reproduced exactly: 16 of 16"),
        "{s}"
    );
    assert!(
        s.contains("dry run: nothing written") && !out.exists(),
        "{s}"
    );
    assert!(!s.contains("22200000") && !s.contains("اسم"), "{s}");
    // Written.
    let s = ok(&omr(&["import", "remark", p(&a), p(&b), "--out", p(&out)]));
    assert!(s.contains("next: omr grade"), "{s}");
    let key = std::fs::read_to_string(out.join("key.csv")).unwrap();
    assert!(key.starts_with("question,A,B,points\r\n"), "{key}");
    assert!(key.contains("\r\n1,") && key.contains(",0.5\r\n"), "{key}");
    assert!(out.join("version_map.csv").exists() && !out.join("clo.csv").exists());
    // F13: the import's files are listed in a signed manifest with their classification and
    // the source workbooks' SHA-384 (§16.3).
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["classification"], "restricted");
    assert_eq!(m["sources"].as_array().unwrap().len(), 2);
    assert!(m["sources"][0]["sha384"].as_str().unwrap().len() == 96);
    assert!(
        m["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "key.csv")
    );
    assert!(
        m["signature"]["value"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(out.join("key.csv"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o077, 0);
    }
    // Two questions every student answers right: refused naming them; --points settles it.
    let c = common::write(
        d.path(),
        "C_Graded.xlsx",
        &xlsx(&book(0, &(0..16).collect::<Vec<_>>(), &[2, 4])),
    );
    let o = omr(&["import", "remark", p(&c), "--dry-run"]);
    assert_eq!(o.status.code(), Some(3));
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(
        e.contains("questions 3, 5") && e.contains("--points"),
        "{e}"
    );
    let pts = common::write(d.path(), "points.csv", b"question,points\n3,0.75\n");
    ok(&omr(&[
        "import",
        "remark",
        p(&c),
        "--points",
        p(&pts),
        "--dry-run",
    ]));
    // Remark's own formats are never opened.
    let aky = common::write(d.path(), "Remark_OMR_Key_A.aky", b"\x00\x01binary");
    let o = omr(&["import", "remark", p(&aky), "--dry-run"]);
    assert_eq!(o.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&o.stderr).contains("never opened"));
}

#[test]
fn compare_writes_the_restricted_report_where_asked() {
    let d = tempfile::tempdir().unwrap();
    let ks: Vec<usize> = (0..4).collect();
    let wa = common::write(d.path(), "A_Graded.xlsx", &xlsx(&book(0, &ks, &[])));
    // Four rows cannot determine eight points: the instructor gives them.
    let pts = common::write(d.path(), "points.csv", POINTS);
    let out = d.path().join("import");
    ok(&omr(&[
        "import",
        "remark",
        p(&wa),
        "--points",
        p(&pts),
        "--out",
        p(&out),
    ]));
    // The four students' sheets, version A, graded with the imported key.
    let e = common::iau();
    let scans = d.path().join("scans");
    for &k in &ks {
        let m = common::Marks::new(&answers(k, 0), &id(k), Some(0));
        common::write(
            &scans,
            &format!("s{k}.png"),
            &common::png(&common::sheet(&e, &m, 7 + k as u64)),
        );
    }
    let exam = d.path().join("exam");
    ok(&omr(&[
        "grade",
        p(&scans),
        "--template",
        "iau-ccsit-v1",
        "--questions",
        "8",
        "--forms",
        "A",
        "--key",
        p(&out.join("key.csv")),
        "--out",
        p(&exam),
        "--quiet",
    ]));
    let report = d.path().join("report");
    let s = ok(&omr(&[
        "compare",
        "remark",
        "--project",
        p(&exam),
        p(&wa),
        "--points",
        p(&pts),
        "--out",
        p(&report),
    ]));
    assert!(s.starts_with("RESTRICTED"), "{s}");
    assert!(s.contains("paired 4 by student ID"), "{s}");
    assert!(s.contains("questions compared 32: equal 32"), "{s}");
    assert!(s.contains("4 settled, 4 equal to Remark's"), "{s}");
    for f in [
        "parallel_run.html",
        "parallel_run_students.csv",
        "parallel_run_questions.csv",
        "parallel_run_identity.csv",
        "parallel_run_summary.json",
        "manifest.json",
    ] {
        assert!(report.join(f).exists(), "{f}");
    }
}
