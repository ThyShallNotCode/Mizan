//! Restricted data at rest and on its way out (PDPL; §14.2, §16.3, §19.3), proven on
//! synthetic IAU sheets only:
//! - SYS-015: a project or an export inside a sync folder is refused, with what to do (SEC-05);
//! - the package and every export are private to the user: folders 0700, files 0600 (SEC-07);
//! - review crops leave the project as a Restricted export with a signed manifest and an
//!   `ExportCreated` event; text shown outside the exports masks student numbers, and showing
//!   them in full is recorded (SEC-09).

mod common;

use common::*;
use omr_app::{ExamProject, KeySource, RosterStudent};
use std::path::Path;

const N: u16 = 12;

fn student(k: u16) -> String {
    format!("22105678{k:02}")
}

fn def() -> omr_app::ExamDefinition {
    let mut d = legacy_def("Quiz", N, &["A"], KeySource::Inferred);
    d.roster = Some(
        (0..3)
            .map(|k| RosterStudent {
                id: student(k),
                name: format!("Student {k}"),
                name_ar: String::new(),
                section: "1".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    d.validate().unwrap();
    d
}

fn scans(dir: &Path) -> std::path::PathBuf {
    let e = iau();
    let out = dir.join("scans");
    for k in 0..3u16 {
        let answers: Vec<Option<u8>> = (0..N).map(|q| Some(((q + k) % 5) as u8)).collect();
        // Sheet 2 carries a stray ID digit: an identity item with a student suggestion.
        let id = if k == 2 {
            "2210567899".to_string()
        } else {
            student(k)
        };
        let m = Marks::new(&answers, &id, Some(0));
        write(
            &out,
            &format!("s{k}.png"),
            &png(&sheet(&e, &m, 40 + u64::from(k))),
        );
    }
    out
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn sync_folders_are_refused_for_projects_and_exports() {
    let d = tempfile::tempdir().unwrap();
    let input = scans(d.path());
    let sync = d.path().join("Dropbox");
    std::fs::create_dir_all(sync.join(".dropbox")).unwrap();
    let r = ExamProject::grade(
        &def(),
        std::slice::from_ref(&input),
        &options(&sync.join("exam"), 2),
        &|_| {},
    );
    let e = r.unwrap_err();
    let t = e.to_string();
    assert!(t.contains("SYS-015") && t.contains("local folder"), "{t}");
    assert!(!sync.join("exam").join("project.omrproj").exists());
    // A project in a safe place cannot export into the sync folder either.
    let mut p = grade(&def(), &[input], &d.path().join("out"), 2);
    let e = p.export(&sync.join("results")).unwrap_err().to_string();
    assert!(e.contains("SYS-015"), "{e}");
    assert!(!sync.join("results").join("results-INFERRED.csv").exists());
    p.close().unwrap();
}

#[cfg(unix)]
#[test]
fn the_package_and_every_export_are_private_to_the_user() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("out");
    let p = grade(&def(), &[scans(d.path())], &out, 2);
    p.close().unwrap();
    let pkg = out.join("project.omrproj");
    assert_eq!(mode(&pkg), 0o700);
    assert_eq!(mode(&out), 0o700, "the export folder the app created");
    assert_eq!(mode(&pkg.join("cache")), 0o700);
    assert_eq!(mode(&pkg.join("cache").join("session.json")), 0o600);
    for f in std::fs::read_dir(&out).unwrap() {
        let f = f.unwrap().path();
        if f.is_file() {
            assert_eq!(mode(&f), 0o600, "{}", f.display());
        }
    }
    // An export into an existing, wider folder still writes private files.
    let shared = d.path().join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    let mut p = ExamProject::open(&pkg, "principal:test").unwrap();
    p.export(&shared).unwrap();
    assert_eq!(mode(&shared.join("manifest.json")), 0o600);
    p.close().unwrap();
}

#[test]
fn review_crops_are_an_audited_restricted_export() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("out");
    let mut p = grade(&def(), &[scans(d.path())], &out, 2);
    let ids: Vec<u64> = p.queue(None).iter().map(|i| i.id).collect();
    assert!(!ids.is_empty());
    let crops = d.path().join("crops");
    let s = p.export_crops(&crops, &ids).unwrap();
    assert!(s.files.iter().any(|f| f.ends_with(".png")));
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(crops.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["purpose"], "review-crops");
    assert_eq!(m["pii"], true);
    assert!(
        m["classification"]
            .as_str()
            .unwrap()
            .starts_with("Restricted")
    );
    assert!(m["signature"]["value"].is_string());
    #[cfg(unix)]
    for f in &s.files {
        assert_eq!(mode(&crops.join(f)), 0o600, "{f}");
    }
    let exports = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "ExportCreated" && e.payload["purpose"] == "review-crops")
        .count();
    assert_eq!(exports, 1);
    // A reveal of personal data outside the exports is recorded (purpose and count only).
    p.record_reveal("review-list", ids.len()).unwrap();
    let ev = p.store().unwrap().events().unwrap().pop().unwrap();
    assert_eq!(ev.event_type, "PersonalDataViewed");
    assert!(!ev.payload.to_string().contains("22105678"));
    p.close().unwrap();
}

#[test]
fn student_numbers_are_masked_outside_the_exports() {
    use omr_app::export::{mask_json, mask_numbers};
    assert_eq!(
        mask_numbers("suggest student 2210567801"),
        "suggest student •••••••801"
    );
    assert_eq!(mask_numbers("Q12 page 3 of 2026"), "Q12 page 3 of 2026");
    let mut v = serde_json::json!({
        "booklet": "bk-0000000000000001",
        "suggestion": "student 2210567801",
        "history": [{"decision": {"Student": "2210567802"}}],
    });
    mask_json(&mut v);
    let t = v.to_string();
    assert!(
        !t.contains("2210567801") && !t.contains("2210567802"),
        "{t}"
    );
    assert_eq!(v["booklet"], "bk-0000000000000001");
}
