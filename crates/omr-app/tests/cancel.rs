//! G-3 D7: the operator cancels a grading run or a survey (synthetic IAU sheets only). The run
//! checks the token before the project package is created, before each page and between
//! stages, never once the store stage has begun: a cancelled new exam stores nothing (its
//! package and the folder made for it are removed), a cancelled append keeps its spool and the
//! same scans add again, a cancel during the store stage is too late (the token says so, and
//! the exam is written whole), and a token nobody cancels never stops a run.

mod common;

use common::*;
use omr_app::define::Sheet;
use omr_app::survey::{SurveyRequest, survey};
use omr_app::{AppError, Cancel, ExamProject, FormKey, KeySource, Progress};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const N: u16 = 20;
const SHEETS: u16 = 6;

fn key(q: u16) -> u8 {
    ((q * 3) % 5) as u8
}

fn sheets(dir: &Path) -> PathBuf {
    sheets_of(dir, 0..SHEETS)
}

/// The sheets `ks` (students `22103458kk`) in `dir`.
fn sheets_of(dir: &Path, ks: std::ops::Range<u16>) -> PathBuf {
    let e = iau();
    for k in ks {
        let answers: Vec<Option<u8>> = (0..N).map(|q| Some((key(q) + (k % 2) as u8) % 5)).collect();
        let m = Marks::new(&answers, &format!("22103458{k:02}"), Some(0));
        write(
            dir,
            &format!("sheet-{k:02}.png"),
            &png(&sheet(&e, &m, 700 + u64::from(k))),
        );
    }
    dir.to_path_buf()
}

fn definition() -> omr_app::ExamDefinition {
    legacy_def(
        "Quiz",
        N,
        &["A"],
        KeySource::Provided {
            forms: vec![FormKey {
                form: "A".into(),
                answers: (0..N)
                    .map(|q| char::from(b'A' + key(q)).to_string())
                    .collect(),
            }],
        },
    )
}

/// What a run reported: pages through the page stage and the stages begun.
#[derive(Default)]
struct Seen {
    started: bool,
    pages: usize,
    stages: Vec<&'static str>,
}

fn is_cancelled(e: &AppError) -> bool {
    matches!(e, AppError::Cancelled)
}

#[test]
fn a_run_cancelled_before_it_starts_creates_no_package() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let def = definition();
    let opts = options(&dir.path().join("out"), 1);
    opts.cancel.cancel();
    let seen = Mutex::new(Seen::default());
    let progress = |p: &Progress| {
        let mut s = seen.lock().unwrap();
        match p {
            Progress::Started { .. } => s.started = true,
            Progress::Page { .. } => s.pages += 1,
            Progress::Stage(st) => s.stages.push(*st),
            Progress::Finished { .. } => {}
        }
    };
    // The pipeline itself (no clean-up around it): the package is never created.
    let pkg = opts.project_dir.clone().unwrap();
    std::fs::create_dir_all(pkg.parent().unwrap()).unwrap();
    let e = omr_app::pipeline::grade(&def, std::slice::from_ref(&scans), &opts, &progress)
        .err()
        .expect("cancelled");
    assert!(is_cancelled(&e), "{e}");
    assert_eq!(
        e.to_string(),
        "cancelled by the operator; nothing was saved"
    );
    assert_eq!(e.exit_code(), 130);
    assert!(!pkg.exists(), "no package, nothing spooled");
    let s = seen.lock().unwrap();
    assert!(!s.started && s.pages == 0 && s.stages.is_empty());
}

#[test]
fn a_cancel_during_the_page_stage_removes_the_package_and_its_new_folder() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let def = definition();
    // A new exam folder: the run creates it, then the package in it.
    let exam_dir = dir.path().join("exams").join("quiz");
    let opts = options(&exam_dir, 1);
    let pkg = opts.project_dir.clone().unwrap();
    let token = opts.cancel.clone();
    let seen = Mutex::new(Seen::default());
    let spooled = Mutex::new(false);
    let progress = |p: &Progress| {
        let mut s = seen.lock().unwrap();
        match p {
            Progress::Started { .. } => s.started = true,
            // The operator presses Cancel while the first page is shown.
            Progress::Page { .. } => {
                s.pages += 1;
                *spooled.lock().unwrap() = pkg.join("project.omr").exists();
                token.cancel();
            }
            Progress::Stage(st) => s.stages.push(*st),
            Progress::Finished { .. } => {}
        }
    };
    let e = ExamProject::grade(&def, std::slice::from_ref(&scans), &opts, &progress)
        .expect_err("cancelled");
    assert!(is_cancelled(&e), "{e}");
    assert!(*spooled.lock().unwrap(), "the originals were spooled first");
    let s = seen.lock().unwrap();
    assert!(s.started);
    assert_eq!(s.pages, 1, "no page is decoded after the cancel");
    assert!(s.stages.is_empty(), "no stage after the page stage");
    assert!(!pkg.exists(), "the package is removed");
    assert!(
        !exam_dir.exists(),
        "the exam folder the run made is removed"
    );
    assert!(
        dir.path().join("exams").exists(),
        "folders it did not make stay"
    );
    // Nothing was kept: the same folder grades from scratch.
    let p = ExamProject::grade(&def, &[scans], &options(&exam_dir, 1), &|_| {}).unwrap();
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS));
}

#[test]
fn a_cancel_between_stages_stops_before_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let def = definition();
    for (at, next) in [
        ("batch", "identity"),
        ("identity", "scoring"),
        ("scoring", "store"),
    ] {
        let out = dir.path().join(format!("out-{at}"));
        let opts = options(&out, 2);
        let token = opts.cancel.clone();
        let stages = Mutex::new(Vec::new());
        let progress = |p: &Progress| {
            if let Progress::Stage(st) = p {
                stages.lock().unwrap().push(*st);
                if *st == at {
                    token.cancel();
                }
            }
        };
        let e = ExamProject::grade(&def, std::slice::from_ref(&scans), &opts, &progress)
            .expect_err("cancelled");
        assert!(is_cancelled(&e), "{at}: {e}");
        let stages = stages.into_inner().unwrap();
        assert_eq!(stages.last(), Some(&at), "{at}: {stages:?}");
        assert!(!stages.contains(&next), "{at}: {stages:?}");
        assert!(!out.exists(), "{at}: nothing stored");
    }
}

#[test]
fn a_cancel_once_the_store_stage_has_begun_is_too_late() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let out = dir.path().join("out");
    let opts = options(&out, 2);
    let token = opts.cancel.clone();
    // what the cancel answered, and whether the run was committed by then
    let answer = Mutex::new(None);
    let progress = |p: &Progress| {
        if matches!(p, Progress::Stage("store")) {
            *answer.lock().unwrap() = Some((token.is_committed(), token.cancel()));
        }
    };
    let p = ExamProject::grade(&definition(), &[scans], &opts, &progress).unwrap();
    assert_eq!(
        answer.into_inner().unwrap(),
        Some((true, false)),
        "committed before the store stage, and the cancel refused"
    );
    assert!(!token.is_cancelled() && token.is_committed());
    assert!(!token.cancel(), "refused again");
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS));
    p.close().unwrap();
    // Written whole: the exam opens and passes the audit checks.
    let p = ExamProject::open(&out.join("project.omrproj"), "principal:test").unwrap();
    p.check_audited().unwrap();
    p.store().unwrap().verify().unwrap();
}

#[test]
fn a_cancel_before_the_last_check_is_accepted_and_never_committed() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let out = dir.path().join("out");
    let opts = options(&out, 2);
    let token = opts.cancel.clone();
    let progress = |p: &Progress| {
        if matches!(p, Progress::Stage("scoring")) {
            assert!(token.cancel(), "in time");
            assert!(token.cancel(), "asked again: still cancelled");
        }
    };
    let e = ExamProject::grade(&definition(), &[scans], &opts, &progress).expect_err("cancelled");
    assert!(is_cancelled(&e), "{e}");
    assert!(token.is_cancelled() && !token.is_committed());
    assert!(!out.exists(), "nothing stored");
}

#[test]
fn a_cancelled_append_keeps_its_spool_and_the_same_scans_add_again() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let more = sheets_of(&dir.path().join("more"), SHEETS..SHEETS + 2);
    let out = dir.path().join("out");
    ExamProject::grade(&definition(), &[scans], &options(&out, 2), &|_| {})
        .unwrap()
        .close()
        .unwrap();
    let pkg = out.join("project.omrproj");
    let mut p = ExamProject::open(&pkg, "principal:test").unwrap();
    // Cancelled while the first new page is read: the batch and its pages are spooled by now.
    let opts = options(&out, 2);
    let token = opts.cancel.clone();
    let progress = |e: &Progress| {
        if matches!(e, Progress::Page { .. }) {
            token.cancel();
        }
    };
    let e = p
        .add_scans(std::slice::from_ref(&more), &opts, &progress)
        .expect_err("cancelled");
    assert!(is_cancelled(&e), "{e}");
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS), "unchanged");
    // The operator adds the same files again: their pages are added once, never refused.
    let r = p
        .add_scans(std::slice::from_ref(&more), &options(&out, 2), &|_| {})
        .unwrap();
    assert_eq!((r.files, r.new_pages, r.duplicates), (2, 2, 0));
    assert_eq!(
        (r.booklets_before, r.booklets_after),
        (usize::from(SHEETS), usize::from(SHEETS) + 2)
    );
    p.close().unwrap();
    let p = ExamProject::open(&pkg, "principal:test").unwrap();
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS) + 2);
    p.check_audited().unwrap();
    p.store().unwrap().verify().unwrap();
    // The cancelled batch stays open in the log; the retry's batch is closed.
    let ev: Vec<String> = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .map(|e| e.event_type)
        .collect();
    let count = |t: &str| ev.iter().filter(|e| *e == t).count();
    assert_eq!((count("BatchOpened"), count("BatchClosed")), (3, 2));
}

#[test]
fn a_token_nobody_cancels_never_stops_a_run_and_clones_share_one_flag() {
    let fresh = Cancel::default();
    assert!(!fresh.is_cancelled());
    assert!(!omr_app::GradeOptions::default().cancel.is_cancelled());
    let held = fresh.clone();
    held.cancel();
    assert!(
        fresh.is_cancelled(),
        "a clone cancels the run that holds the token"
    );
    assert!(
        !Cancel::default().is_cancelled(),
        "another token is untouched"
    );
    assert!(fresh == held && fresh != Cancel::default());
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let p = ExamProject::grade(
        &definition(),
        &[scans],
        &options(&dir.path().join("out"), 2),
        &|_| {},
    )
    .unwrap();
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS));
}

#[test]
fn a_survey_honours_the_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let scans = sheets(&dir.path().join("scans"));
    let req = SurveyRequest {
        sheet: Sheet::Template("iau-ccsit-v1".into()),
        scans: vec![scans],
        key_sheets: Vec::new(),
        roster: None,
        threads: 1,
        cancel: Cancel::default(),
    };
    // Cancelled while its first page is read.
    let pages = Mutex::new(0);
    let progress = |p: &Progress| {
        if matches!(p, Progress::Page { .. }) {
            *pages.lock().unwrap() += 1;
            req.cancel.cancel();
        }
    };
    let e = survey(&req, &progress).expect_err("cancelled");
    assert!(is_cancelled(&e), "{e}");
    assert_eq!(*pages.lock().unwrap(), 1);
    // A survey with a fresh token runs to its end.
    let req = SurveyRequest {
        cancel: Cancel::default(),
        ..req.clone()
    };
    let s = survey(&req, &|_| {}).unwrap();
    assert_eq!(s.booklets, usize::from(SHEETS));
}
