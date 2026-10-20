//! The desktop workflow without the command line (G-3): file dialogs on every path field,
//! opening another exams folder, the file manager, and the settings kept between starts. The
//! window gets what the binary supplies from the operating system through a fake host (it
//! records each dialog asked for and answers from a queue, each folder shown and each save of
//! the preferences), and is driven as a user does: real pointer clicks and typed keys, on
//! synthetic exams (no real data).
//!
//! Each test runs on its own thread with its own Slint testing platform.

mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use omr_app::ExamProject;
use shell_slint::app_backend::AppBackend;
use shell_slint::backend::{Backend, ExamId};
use shell_slint::host::{PickKind, PickTarget, Prefs};
use shell_slint::ui::{App, AppWindow, ExamsVM, RemarkVM, ResultsVM, ScanVM, SetupVM, Theme};
use slint::ComponentHandle;
use slint::platform::Key;
use std::path::{Path, PathBuf};
use support::a11::{by_label, chord, click, one, special, tab, type_text, with_prefix};
use support::harness::{
    FakeHost, Harness, Held, grade, harness, harness_in, held_spawner, run_held,
};
use support::synthetic_exam::{self as synth, MIDTERM};
use support::synthetic_remark as remark;

/// An exams folder (`exams`) beside the scans of six synthetic sheets (`scans`), the window on
/// its Exams screen with the work it starts held.
struct Workspace {
    h: Harness,
    held: Held,
    ws: PathBuf,
    scans: PathBuf,
}

fn workspace(dir: &Path) -> Workspace {
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.join("scans"));
    let ws = dir.join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11");
    app.set_spawner(spawn);
    let h = harness_in(app);
    Workspace { h, held, ws, scans }
}

fn message(ui: &AppWindow) -> String {
    ui.global::<App>().get_message().to_string()
}

/// Clicks into the field labelled `label` and types `text`, key by key.
fn type_into(ui: &AppWindow, label: &str, text: &str) {
    click(ui, &one(ui, label));
    type_text(ui, text);
}

/// Clicks the button labelled `label`.
fn press(ui: &AppWindow, label: &str) {
    click(ui, &one(ui, label));
}

/// The labels of every Browse… and Folder… button on show.
fn browse_buttons(ui: &AppWindow) -> Vec<String> {
    let mut v: Vec<String> = with_prefix(ui, "Browse for ")
        .into_iter()
        .chain(with_prefix(ui, "Choose a folder for "))
        .map(|(l, _)| l)
        .collect();
    v.sort();
    v
}

fn texts(list: &[&str]) -> Vec<String> {
    list.iter().map(|e| e.to_string()).collect()
}

/// D1: without file dialogs (headless, screenshots) no Browse button exists and typed paths
/// grade as before; given a host, every path field of the New-exam form has its buttons.
#[test]
fn no_host_hides_browse_buttons() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    assert!(ui.global::<ExamsVM>().get_new_open(), "the form opens");
    assert!(!ui.global::<App>().get_can_browse());
    assert_eq!(browse_buttons(ui), Vec::<String>::new());
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    type_into(ui, "Number of questions", "30");
    press(ui, "Grade exam");
    let msg = message(ui);
    assert!(msg.contains("Grading 6 scan files"), "{msg}");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    let d = w.h.app.borrow().dashboard();
    assert_eq!(d.exams.len(), 1);
    assert_eq!(d.exams[0].sheets_scanned, 6);

    // the desktop app's window: Browse… beside each path, Folder… for the scans
    w.h.shell.set_host(FakeHost::new().host(Prefs::default()));
    assert!(ui.global::<App>().get_can_browse());
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    assert_eq!(
        browse_buttons(ui),
        texts(&[
            "Browse for Answer key file",
            "Browse for CLO mapping file",
            "Browse for Key sheets",
            "Browse for Roster file",
            "Browse for Scan files or folders",
            "Browse for Sheet template or spec file",
            "Choose a folder for Scan files or folders",
        ])
    );
}

/// D2: the scans picked in the dialog are graded exactly as picked, a `;` in a file name
/// included (typed, it would separate two paths); the dialog offers the scan types, opens in
/// the exams folder first and, from then on, where the scans were picked (saved).
#[test]
fn browse_fills_the_scans_field_with_the_picked_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    // a scanner's file name may hold a ;
    let odd = w.scans.join("batch;1.jpg");
    std::fs::rename(w.scans.join("scan-001.jpg"), &odd).expect("rename");
    let first = w.scans.join("scan-000.jpg");
    let last = w.scans.join("scan-002.jpg");
    press(ui, "New exam");
    host.will_pick(&[&first, &odd, &last]);
    press(ui, "Browse for Scan files or folders");
    let req = host.last_request();
    assert_eq!(req.target, PickTarget::NewScans);
    assert_eq!(req.kind, PickKind::Files);
    let (name, exts) = &req.filters[0];
    assert_eq!(name, "Scans");
    for e in ["pdf", "tif", "tiff", "jpg", "jpeg", "png"] {
        assert!(exts.iter().any(|x| x == e), "{e} offered: {exts:?}");
    }
    assert_eq!(
        req.start.as_deref(),
        Some(w.ws.as_path()),
        "the exams folder"
    );
    let shown = ui.global::<ExamsVM>().get_new_scans().to_string();
    assert_eq!(
        shown,
        format!("{}; {}; {}", first.display(), odd.display(), last.display())
    );
    let saved = host
        .saved
        .borrow()
        .last()
        .cloned()
        .expect("preferences saved");
    assert_eq!(saved.last_dirs.get("scans"), Some(&w.scans));

    type_into(ui, "Number of questions", "30");
    press(ui, "Grade exam");
    let msg = message(ui);
    assert!(msg.contains("Grading 3 scan files"), "{msg}");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    let d = w.h.app.borrow().dashboard();
    assert_eq!(d.exams[0].sheets_scanned, 3);

    // the next exam's scans: a folder this time, the dialog opens where the scans were
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    press(ui, "Choose a folder for Scan files or folders");
    let req = host.last_request();
    assert_eq!(req.target, PickTarget::NewScanFolder);
    assert_eq!(req.kind, PickKind::Folder);
    assert!(req.filters.is_empty());
    assert_eq!(req.start.as_deref(), Some(w.scans.as_path()));
}

/// A dialog closed without a choice leaves what was typed, says once what to do when no dialog
/// appears, and the next click opens a dialog again.
#[test]
fn a_cancelled_dialog_keeps_the_typed_text() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", "~/typed scans");
    host.will_cancel();
    press(ui, "Browse for Scan files or folders");
    assert_eq!(host.requests.borrow().len(), 1);
    assert_eq!(ui.global::<ExamsVM>().get_new_scans(), "~/typed scans");
    let msg = message(ui);
    assert!(
        msg.contains("Nothing was chosen") && msg.contains("type the path"),
        "{msg}"
    );
    assert!(host.saved.borrow().is_empty(), "nothing to remember");
    press(ui, "Browse for Scan files or folders");
    assert_eq!(host.requests.borrow().len(), 2, "the dialog closed");
    assert_eq!(ui.global::<ExamsVM>().get_new_scans(), "~/typed scans");
}

/// Picked paths are used only while the field shows them: edited, it is read as typed.
#[test]
fn editing_a_picked_field_falls_back_to_typed_paths() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam");
    host.will_pick(&[&w.scans.join("scan-000.jpg"), &w.scans.join("scan-001.jpg")]);
    press(ui, "Browse for Scan files or folders");
    assert!(ui.global::<ExamsVM>().get_new_scans().contains("; "));
    // the whole folder instead, typed over the picked files
    click(ui, &one(ui, "Scan files or folders"));
    chord(ui, Key::Control, "a");
    type_text(ui, w.scans.to_str().unwrap());
    assert_eq!(
        ui.global::<ExamsVM>().get_new_scans(),
        w.scans.to_str().unwrap()
    );
    type_into(ui, "Number of questions", "30");
    press(ui, "Grade exam");
    let msg = message(ui);
    assert!(msg.contains("Grading 6 scan files"), "{msg}");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().dashboard().exams[0].sheets_scanned, 6);
}

/// While a dialog is open, Browse clicks (of any field) open no other; its answer fills the
/// field it was opened for. A dialog that could not show at all does not block the next.
#[test]
fn browse_clicks_wait_for_the_open_dialog() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam");
    host.hold.set(true);
    press(ui, "Browse for Answer key file");
    press(ui, "Browse for Answer key file");
    press(ui, "Browse for Roster file");
    assert_eq!(host.requests.borrow().len(), 1, "one dialog at a time");
    let key = w.ws.join("key.csv");
    host.will_pick(&[&key]);
    host.close();
    let ex = ui.global::<ExamsVM>();
    assert_eq!(ex.get_new_key(), key.to_str().unwrap());
    assert_eq!(ex.get_new_roster(), "");
    press(ui, "Browse for Roster file");
    assert_eq!(host.requests.borrow().len(), 2);
    assert_eq!(host.last_request().target, PickTarget::NewRoster);
    host.fail_held();
    assert_eq!(ex.get_new_roster(), "");
    press(ui, "Browse for Roster file");
    assert_eq!(host.requests.borrow().len(), 3, "the failed dialog closed");
}

/// Each file field of the New-exam form offers its own file types; the Remark import panel
/// offers Remark's graded exports only (its own files are never opened).
#[test]
fn roster_and_clo_browse_use_their_filters() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam");
    let cases: [(&str, PickTarget, PickKind, &str, &[&str]); 5] = [
        (
            "Answer key file",
            PickTarget::NewKey,
            PickKind::File,
            "Key file",
            &["csv", "tsv", "txt"],
        ),
        (
            "Roster file",
            PickTarget::NewRoster,
            PickKind::File,
            "Roster",
            &["csv", "xlsx"],
        ),
        (
            "CLO mapping file",
            PickTarget::NewClo,
            PickKind::File,
            "CLO file",
            &["csv"],
        ),
        (
            "Key sheets",
            PickTarget::NewKeySheets,
            PickKind::Files,
            "Scans",
            omr_app::ingest::EXTENSIONS,
        ),
        (
            "Sheet template or spec file",
            PickTarget::NewSpec,
            PickKind::File,
            "Sheet spec",
            &["json"],
        ),
    ];
    for (label, target, kind, filter, exts) in cases {
        let file = dir.path().join(format!("{}.{}", target.name(), exts[0]));
        host.will_pick(&[&file]);
        press(ui, &format!("Browse for {label}"));
        let req = host.last_request();
        assert_eq!((req.target, req.kind), (target, kind), "{label}");
        assert_eq!(req.filters[0], (filter.to_string(), texts(exts)), "{label}");
        assert_eq!(
            req.filters.last().map(|f| f.0.as_str()),
            Some("All files"),
            "{label}"
        );
        assert_eq!(
            one(ui, label).accessible_value().unwrap_or_default(),
            file.to_str().unwrap(),
            "{label} shows the picked file"
        );
    }
    let ex = ui.global::<ExamsVM>();
    assert!(ex.get_new_roster().ends_with("new-roster.csv"));
    assert!(ex.get_new_clo().ends_with("new-clo.csv"));

    press(ui, "Import from Remark export…");
    assert!(ui.global::<RemarkVM>().get_new_panel());
    press(ui, "Browse for Remark workbooks");
    let req = host.last_request();
    assert_eq!(
        (req.target, req.kind),
        (PickTarget::RemarkBooks, PickKind::Files)
    );
    assert_eq!(
        req.filters,
        vec![("Remark exports".to_string(), texts(&["xlsx"]))]
    );
    press(ui, "Browse for Points file (optional)");
    assert_eq!(host.last_request().target, PickTarget::RemarkPoints);
}

/// Setup's key file is read as one path: quoted as Explorer's "Copy as path" gives it, or
/// picked in the dialog.
#[test]
fn setup_replace_key_accepts_a_quoted_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let project = dir.path().join("bare-quiz/project.omrproj");
    let def = synth::definition(&spec);
    grade(&def, &scans, &project);
    // the instructor's key file (one column per form), and a corrected one
    let key_csv = |first: u8| {
        let mut csv = format!("question,{}\n", def.forms.join(","));
        for q in 0..def.questions {
            csv.push_str(&(q + 1).to_string());
            for f in 0..def.forms.len() {
                let k = if q == 0 {
                    first
                } else {
                    synth::key(&spec, f, q)
                };
                csv.push(',');
                csv.push(char::from(b'A' + k));
            }
            csv.push('\n');
        }
        csv
    };
    let key = dir.path().join("key.csv");
    std::fs::write(&key, key_csv(synth::key(&spec, 0, 0))).unwrap();
    let fixed_dir = dir.path().join("keys; corrected");
    std::fs::create_dir_all(&fixed_dir).unwrap();
    let fixed = fixed_dir.join("key.csv");
    std::fs::write(&fixed, key_csv((synth::key(&spec, 0, 0) + 1) % 5)).unwrap();

    let h = harness(AppBackend::project(&project, "principal:a11").expect("open"));
    let ui = &h.ui;
    let host = FakeHost::new();
    h.shell.set_host(host.host(Prefs::default()));
    click(ui, &tab(ui, "Setup"));
    press(ui, "Replace key…");
    assert!(ui.global::<SetupVM>().get_key_open());
    type_into(ui, "Key file", &format!("\"{}\"", key.display()));
    press(ui, "Replace and rescore");
    let msg = message(ui);
    assert!(
        msg.contains("in force") && msg.contains("rescored"),
        "{msg}"
    );
    let label = h.app.borrow().results(&h.exam).expect("results").key_label;
    assert!(!label.to_uppercase().contains("INFERRED"), "{label}");

    press(ui, "Replace key…");
    host.will_pick(&[&fixed]);
    press(ui, "Browse for Key file");
    let req = host.last_request();
    assert_eq!(
        (req.target, req.kind),
        (PickTarget::SetupKey, PickKind::File)
    );
    assert_eq!(
        req.filters[0],
        ("Key file".to_string(), texts(&["csv", "tsv", "txt"]))
    );
    assert_eq!(
        ui.global::<SetupVM>().get_key_path(),
        fixed.to_str().unwrap()
    );
    let before = msg;
    press(ui, "Replace and rescore");
    let msg = message(ui);
    assert!(msg.contains("in force") && msg != before, "{msg}");
}

/// Setup (key sheets, CLO file, Remark import), Scan (added scans, files or a folder) and the
/// parallel run (Remark's workbooks, the report folder) take picked paths too; picked
/// workbooks in a folder whose name holds a `;` are compared as picked.
#[test]
fn setup_scan_and_parallel_run_fields_browse() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    let books = remark::write_books_with(&MIDTERM, &dir.path().join("remark; October"), true);
    let reports = dir.path().join("reports; October");
    std::fs::create_dir_all(&reports).unwrap();
    let app = AppBackend::project(&built.exam_dir, "principal:a11-instructor").expect("open");
    let h = harness_in(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 2400));
    let host = FakeHost::new();
    h.shell.set_host(host.host(Prefs::default()));

    click(ui, &tab(ui, "Setup"));
    press(ui, "Browse for CLO file");
    assert_eq!(
        host.last_request().filters[0],
        ("CLO file".to_string(), texts(&["csv"]))
    );
    press(ui, "Read key from answer sheet…");
    let key_sheet = built.exam_dir.join("key-sheet.jpg");
    host.will_pick(&[&key_sheet]);
    press(ui, "Browse for Key-sheet files");
    let req = host.last_request();
    assert_eq!(
        (req.target, req.kind),
        (PickTarget::SetupKeySheets, PickKind::Files)
    );
    assert_eq!(req.filters[0].0, "Scans");
    assert_eq!(
        ui.global::<SetupVM>().get_ks_path(),
        key_sheet.to_str().unwrap()
    );
    press(ui, "Import from Remark export…");
    press(ui, "Browse for Remark workbooks");
    assert_eq!(host.last_request().target, PickTarget::RemarkBooks);

    click(ui, &tab(ui, "Scan"));
    press(ui, "Add scans…");
    press(ui, "Browse for Scan files or folders to add");
    assert_eq!(
        (host.last_request().target, host.last_request().kind),
        (PickTarget::AddScans, PickKind::Files)
    );
    let day2 = dir.path().join("day 2");
    host.will_pick(&[&day2]);
    press(ui, "Choose a folder for Scan files or folders to add");
    assert_eq!(host.last_request().kind, PickKind::Folder);
    assert_eq!(ui.global::<ScanVM>().get_add_path(), day2.to_str().unwrap());

    click(ui, &tab(ui, "Results"));
    press(ui, "Compare with Remark export…");
    let picked: Vec<&Path> = books.iter().map(PathBuf::as_path).collect();
    host.will_pick(&picked);
    press(ui, "Browse for Remark workbooks to compare");
    assert_eq!(host.last_request().target, PickTarget::CompareBooks);
    press(ui, "Compare");
    let rm = ui.global::<RemarkVM>();
    assert!(
        rm.get_cmp_open() && !rm.get_cmp_refused(),
        "compared as picked: {}",
        message(ui)
    );
    host.will_pick(&[&reports]);
    press(ui, "Browse for Report folder");
    assert_eq!(
        (host.last_request().target, host.last_request().kind),
        (PickTarget::ParallelDir, PickKind::Folder)
    );
    press(ui, "Save report");
    assert!(
        message(ui).contains("Wrote the Restricted parallel-run report"),
        "{}",
        message(ui)
    );
    for f in omr_app::parallel::REPORT_FILES {
        assert!(reports.join(f).exists(), "{f} in the picked folder");
    }
}

/// Every Browse… and Folder… button on show sits inside the window, and leaves its field room
/// for a path.
fn buttons_inside(ui: &AppWindow, what: &str) {
    let width = ui.window().size().width as f32;
    let height = ui.window().size().height as f32;
    let buttons: Vec<_> = with_prefix(ui, "Browse for ")
        .into_iter()
        .map(|(l, e)| (l["Browse for ".len()..].to_string(), e))
        .chain(
            with_prefix(ui, "Choose a folder for ")
                .into_iter()
                .map(|(l, e)| (l["Choose a folder for ".len()..].to_string(), e)),
        )
        .collect();
    assert!(!buttons.is_empty(), "{what}: no Browse button");
    for (field, b) in buttons {
        let (p, sz) = (b.absolute_position(), b.size());
        assert!(
            p.x >= 0.0 && p.x + sz.width <= width + 0.5 && p.y + sz.height <= height + 0.5,
            "{what}: the button of {field:?} ends at ({}, {}) past the window",
            p.x + sz.width,
            p.y + sz.height
        );
        let f = one(ui, &field);
        assert!(
            f.size().width >= 120.0,
            "{what}: {field:?} is {} px wide beside its buttons",
            f.size().width
        );
    }
}

/// With file dialogs, every screen's path fields and their buttons still fit the smallest
/// window, in both themes.
#[test]
fn browse_buttons_fit_the_smallest_window() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    let books = remark::write_books_with(&MIDTERM, &dir.path().join("remark"), true);
    let ws = built.exam_dir.parent().map(Path::to_path_buf).expect("ws");
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11-instructor");
    app.set_spawner(spawn);
    let h = harness_in(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    h.shell.set_host(FakeHost::new().host(Prefs::default()));
    let (ex, su, rm) = (
        ui.global::<ExamsVM>(),
        ui.global::<SetupVM>(),
        ui.global::<RemarkVM>(),
    );
    for dark in [false, true] {
        ui.global::<shell_slint::ui::Theme>().set_dark(dark);
        let what = |screen: &str| format!("{screen}, dark {dark}");
        click(ui, &tab(ui, "Exams"));
        press(ui, "New exam");
        buttons_inside(ui, &what("New exam"));
        press(ui, "Import from Remark export…");
        buttons_inside(ui, &what("New exam, Remark import"));
        rm.set_new_panel(false);
        ex.set_new_open(false);

        click(ui, &tab(ui, "Setup"));
        buttons_inside(ui, &what("Setup"));
        press(ui, "Replace key…");
        buttons_inside(ui, &what("Setup, replace key"));
        su.set_key_open(false);
        press(ui, "Read key from answer sheet…");
        buttons_inside(ui, &what("Setup, key sheets"));
        su.set_ks_open(false);

        click(ui, &tab(ui, "Scan"));
        press(ui, "Add scans…");
        buttons_inside(ui, &what("Scan, add scans"));
        ui.global::<ScanVM>().set_add_open(false);

        click(ui, &tab(ui, "Results"));
        press(ui, "Compare with Remark export…");
        buttons_inside(ui, &what("Results, compare"));
        h.shell.remark_compare(
            &books
                .iter()
                .map(|b| b.display().to_string())
                .collect::<Vec<_>>()
                .join(";"),
        );
        assert!(rm.get_cmp_open(), "{}", message(ui));
        buttons_inside(ui, &what("Results, parallel run"));
        press(ui, "Back to results");
    }
}

// ------------------------------------------------------------------ exams folders (D3, D4)

/// A small exam (six sheets, the key inferred): CS 213 Quiz 1.
const QUIZ_1: synth::Spec = synth::Spec {
    sheets: 6,
    ..synth::BARE
};

/// Another small exam, graded earlier than [`QUIZ_1`]: CS 299 Quiz 9.
const QUIZ_9: synth::Spec = synth::Spec {
    course_code: "CS 299",
    course_title: "Compilers",
    course_title_ar: "المترجمات",
    title: "Quiz 9",
    date: "2026-09-30",
    snapshot: "2026-10-01T08:00:00Z",
    sheets: 6,
    seed: 20_261_020,
    folder: "cs299-quiz-9",
    ..synth::BARE
};

/// An exams folder holding the exams `specs`, graded from their synthetic sheets.
fn exams_folder(dir: &Path, specs: &[&synth::Spec]) -> PathBuf {
    std::fs::create_dir_all(dir).expect("exams folder");
    for s in specs {
        synth::build(s, dir);
    }
    dir.to_path_buf()
}

/// The window on the exams folder `ws`, with file dialogs and the file manager (the fake host),
/// the work it starts held.
fn window_on(ws: &Path) -> (Harness, Held, std::rc::Rc<FakeHost>) {
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(ws, "principal:a11");
    app.set_spawner(spawn);
    let h = harness_in(app);
    let host = FakeHost::new();
    h.shell.set_host(host.host(Prefs::default()));
    (h, held, host)
}

/// The course codes of the exams table, top to bottom.
fn courses(ui: &AppWindow) -> Vec<String> {
    use slint::Model;
    let rows = ui.global::<ExamsVM>().get_rows();
    rows.iter().map(|r| r.code.to_string()).collect()
}

/// The navigation tab whose label starts with `label` (Review carries its open count).
fn tab_of(ui: &AppWindow, label: &str) -> ElementHandle {
    let tabs: Vec<_> = with_prefix(ui, label)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Tab))
        .collect();
    assert_eq!(tabs.len(), 1, "one tab labelled {label}…");
    tabs.into_iter().next().unwrap()
}

/// The preferences as the last save gave them.
fn last_saved(host: &FakeHost) -> Prefs {
    let saved = host.saved.borrow().last().cloned();
    saved.expect("the preferences were saved")
}

/// The exam's last event recorded by a person (engines record their own work after it).
fn last_actor(h: &Harness, exam: &ExamId) -> String {
    h.app
        .borrow()
        .with_project(exam, |p| {
            let events = p.store().expect("a store").events().expect("events");
            let last = events
                .iter()
                .rev()
                .find(|e| !e.actor_id.starts_with("engine:"));
            last.map(|e| e.actor_id.clone()).unwrap_or_default()
        })
        .expect("the exam is open")
}

/// D3: Open folder… (and Ctrl+O) chooses another exams folder in the folder dialog: its exams
/// are listed, the first one selected, and it is the folder opened next time.
#[test]
fn open_folder_lists_the_other_folders_exams_and_remembers_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fall = exams_folder(&dir.path().join("fall"), &[&QUIZ_1]);
    let spring = exams_folder(&dir.path().join("spring"), &[&QUIZ_9]);
    let (h, _held, host) = window_on(&fall);
    let ui = &h.ui;
    assert_eq!(courses(ui), ["CS 213"]);

    host.will_pick(&[&spring]);
    press(ui, "Open folder…");
    let req = host.last_request();
    assert_eq!(
        (req.target, req.kind),
        (PickTarget::Workspace, PickKind::Folder)
    );
    assert!(req.filters.is_empty());
    assert_eq!(courses(ui), ["CS 299"], "the other folder's exams");
    let app = ui.global::<App>();
    assert!(app.get_course().starts_with("CS 299"), "its exam is open");
    assert!(app.get_workspace().ends_with("spring"));
    let msg = message(ui);
    assert!(msg.contains("Opened") && msg.contains("1 exam"), "{msg}");
    let saved = last_saved(&host);
    assert_eq!(saved.workspace.as_deref(), Some(spring.as_path()));
    assert_eq!(saved.recent.first(), Some(&spring));
    // the next dialog opens beside the folder chosen
    assert_eq!(
        saved.last_dirs.get("folders"),
        Some(&dir.path().to_path_buf())
    );

    // Ctrl+O on the Exams screen; a dialog closed without a choice changes nothing
    host.will_cancel();
    chord(ui, Key::Control, "o");
    assert_eq!(host.requests.borrow().len(), 2, "Ctrl+O opens the dialog");
    assert_eq!(host.last_request().target, PickTarget::Workspace);
    assert_eq!(courses(ui), ["CS 299"]);
    // back to the first folder
    host.will_pick(&[&fall]);
    chord(ui, Key::Control, "o");
    assert_eq!(courses(ui), ["CS 213"]);
    let saved = last_saved(&host);
    assert_eq!(saved.workspace.as_deref(), Some(fall.as_path()));
    assert_eq!(saved.recent, vec![fall.clone(), spring.clone()]);
}

/// D3: choosing the folder of one exam opens the folder it sits in, with that exam selected
/// (not the newest).
#[test]
fn open_exam_folder_selects_that_exam() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1, &QUIZ_9]);
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let (h, _held, host) = window_on(&empty);
    let ui = &h.ui;
    assert!(courses(ui).is_empty());
    host.will_pick(&[&ws.join(QUIZ_9.folder)]);
    press(ui, "Open folder…");
    assert_eq!(courses(ui), ["CS 213", "CS 299"], "newest first");
    let ex = ui.global::<ExamsVM>();
    assert_eq!(ex.get_selected(), 1, "the exam chosen is selected");
    assert!(ex.get_drawer_open());
    assert_eq!(ex.get_drawer().code, "CS 299");
    assert!(ui.global::<App>().get_course().starts_with("CS 299"));
    let msg = message(ui);
    assert!(msg.contains("CS 299 Quiz 9 is selected"), "{msg}");
    assert_eq!(last_saved(&host).workspace.as_deref(), Some(ws.as_path()));

    // its project.omrproj chosen works the same; a folder with no exam is refused as it is
    host.will_pick(&[&ws.join(QUIZ_1.folder).join("project.omrproj")]);
    press(ui, "Open folder…");
    assert_eq!(ex.get_drawer().code, "CS 213");
    host.will_pick(&[&dir.path().join("nowhere")]);
    press(ui, "Open folder…");
    let msg = message(ui);
    assert!(msg.contains("no such folder"), "{msg}");
    assert_eq!(courses(ui), ["CS 213", "CS 299"], "nothing changed");
    assert_eq!(
        last_saved(&host).workspace.as_deref(),
        Some(ws.as_path()),
        "the folder opened next time is still the last one opened"
    );
}

/// D3: Refresh (F5) reads the folder again: an exam graded meanwhile (here, as the command
/// line would) is listed, and one whose folder was removed goes.
#[test]
fn refresh_shows_an_exam_graded_behind_the_apps_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, _held, _host) = window_on(&ws);
    let ui = &h.ui;
    assert_eq!(courses(ui), ["CS 213"]);
    synth::build(&QUIZ_9, &ws);
    assert_eq!(courses(ui), ["CS 213"], "not read yet");
    special(ui, Key::F5);
    assert_eq!(courses(ui), ["CS 213", "CS 299"]);
    let msg = message(ui);
    assert!(msg.contains("2 exams") && msg.contains("1 new"), "{msg}");
    assert_eq!(
        ui.global::<ExamsVM>().get_drawer().code,
        "CS 213",
        "the exam open stays selected"
    );

    std::fs::remove_dir_all(ws.join(QUIZ_9.folder)).unwrap();
    press(ui, "Refresh the exams list (F5)");
    assert_eq!(courses(ui), ["CS 213"]);
    let msg = message(ui);
    assert!(msg.contains("1 no longer in the folder"), "{msg}");
    press(ui, "Refresh the exams list (F5)");
    assert!(message(ui).contains("nothing changed"), "{}", message(ui));
}

/// D3: while an exam is being graded the folder stays: opening another is refused with what to
/// do, and works once the run has ended.
#[test]
fn opening_a_folder_is_refused_while_grading() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    type_into(ui, "Number of questions", "30");
    press(ui, "Grade exam");
    assert!(w.h.app.borrow().grading_in_progress());
    click(ui, &tab(ui, "Exams"));
    host.will_pick(&[&other]);
    press(ui, "Open folder…");
    let msg = message(ui);
    assert!(
        msg.contains("is being graded") && msg.contains("when the Scan screen says"),
        "{msg}"
    );
    assert_eq!(courses(ui).len(), 1, "the run's exam is still listed");
    assert!(ui.global::<App>().get_workspace().ends_with("exams"));
    assert_eq!(
        last_saved(&host).workspace,
        None,
        "nothing opened, nothing remembered"
    );

    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    host.will_pick(&[&other]);
    press(ui, "Open folder…");
    let msg = message(ui);
    assert!(msg.contains("no exams in it yet"), "{msg}");
    assert!(courses(ui).is_empty());
    one(ui, "No exams in this folder yet");
}

/// D3: another folder opened saves and closes the exams open: the project is free for another
/// program and its signed checkpoint covers the last decision.
#[test]
fn switching_folders_closes_open_projects() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fall = exams_folder(&dir.path().join("fall"), &[&QUIZ_1]);
    let spring = dir.path().join("spring");
    std::fs::create_dir_all(&spring).unwrap();
    let (h, _held, host) = window_on(&fall);
    let ui = &h.ui;
    let package = fall.join(QUIZ_1.folder).join("project.omrproj");
    let checkpoint = |p: &ExamProject| {
        let st = p.store().expect("a store");
        st.latest_checkpoint().expect("readable").map(|c| c.seq)
    };
    let graded = h
        .app
        .borrow()
        .with_project(&h.exam, checkpoint)
        .flatten()
        .expect("grading ended with a checkpoint");
    // a decision after it
    click(ui, &tab_of(ui, "Review"));
    press(ui, "Accept suggestion");
    let head = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            p.store().unwrap().events().unwrap().last().map(|e| e.seq)
        })
        .flatten()
        .expect("events");
    assert!(
        head > graded,
        "the decision is recorded after the checkpoint"
    );
    // held open here: another program could not write it
    assert!(ExamProject::open(&package, "principal:other").is_err());

    click(ui, &tab(ui, "Exams"));
    host.will_pick(&[&spring]);
    press(ui, "Open folder…");
    assert!(message(ui).contains("Opened"), "{}", message(ui));
    let p = ExamProject::open(&package, "principal:other").expect("free to open read-write");
    let closed = checkpoint(&p).expect("a checkpoint");
    assert!(
        closed >= head,
        "the close signed a checkpoint over the decision: {closed} < {head}"
    );
}

/// The `--project` exam's folder is not the exams folder: New exam grades beside it.
#[test]
fn project_mode_grades_new_exams_beside_the_exam_not_inside_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let exam_dir = ws.join(QUIZ_1.folder);
    let scans = synth::write_scans(&QUIZ_9, &dir.path().join("scans"));
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::project(&exam_dir, "principal:a11").expect("open");
    app.set_spawner(spawn);
    assert_eq!(app.workspace_dir().as_deref(), Some(ws.as_path()));
    let h = harness(app);
    let ui = &h.ui;
    assert!(ui.global::<App>().get_workspace().ends_with("exams"));
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", scans.to_str().unwrap());
    type_into(ui, "Number of questions", "30");
    type_into(ui, "Course code", "CS 299");
    press(ui, "Grade exam");
    let msg = message(ui);
    assert!(msg.contains("Grading 6 scan files"), "{msg}");
    assert!(
        !msg.contains(&format!(
            "{}{}",
            exam_dir.display(),
            std::path::MAIN_SEPARATOR
        )),
        "{msg}"
    );
    run_held(&h, &held);
    assert_eq!(h.app.borrow().grading_error(), None);
    let graded: Vec<PathBuf> = std::fs::read_dir(&ws)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("project.omrproj").is_dir())
        .collect();
    assert_eq!(graded.len(), 2, "the new exam beside the first: {graded:?}");
    assert!(
        std::fs::read_dir(&exam_dir)
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| !e.path().join("project.omrproj").exists()),
        "nothing graded inside the exam's folder"
    );
}

/// D3: the drawer shows the exam's folder (its project and exports) in the file manager.
#[test]
fn drawer_open_exam_folder_reveals_the_exam_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11");
    app.set_spawner(spawn);
    let h = harness_in(app);
    let ui = &h.ui;
    assert!(ui.global::<ExamsVM>().get_drawer_open());
    assert!(
        by_label(ui, "Open exam folder").is_empty(),
        "no file manager, no button"
    );
    let host = FakeHost::new();
    h.shell.set_host(host.host(Prefs::default()));
    press(ui, "Open exam folder");
    let shown = host.revealed.borrow().clone();
    assert_eq!(shown, vec![ws.join(QUIZ_1.folder)]);
    assert!(shown[0].join("project.omrproj").is_dir());
    assert!(message(ui).contains("Opened"), "{}", message(ui));
}

/// D3: Results' export bar writes the exports and shows the folder they are in.
#[test]
fn results_open_exports_folder_reveals_the_folder_with_results_csv() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, _held, host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Results"));
    press(ui, "Export…");
    press(ui, "Write all exports");
    assert!(message(ui).contains("Wrote"), "{}", message(ui));
    press(ui, "Open exports folder");
    let shown = host.revealed.borrow().clone();
    assert_eq!(shown, vec![ws.join(QUIZ_1.folder)]);
    let results = std::fs::read_dir(&shown[0])
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("results") && n.ends_with(".csv"))
        .count();
    assert_eq!(results, 1, "the results file is in the folder shown");
}

/// Only folders reach the file manager: the exams folder from Settings, never a file; an exam
/// being graded has no folder yet, so nothing is shown for it.
#[test]
fn reveal_never_receives_a_file_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    // an exam being graded: selected, but no folder to show
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    type_into(ui, "Number of questions", "30");
    press(ui, "Grade exam");
    click(ui, &tab(ui, "Exams"));
    press(ui, "Open exam folder");
    assert!(host.revealed.borrow().is_empty());
    assert!(message(ui).contains("no folder yet"), "{}", message(ui));
    run_held(&w.h, &w.held);
    press(ui, "Open exam folder");
    click(ui, &tab(ui, "Settings"));
    let label = ui.global::<App>().get_reveal_label().to_string();
    let expected = if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Show folder"
    };
    assert_eq!(label, expected);
    press(ui, &label);
    let shown = host.revealed.borrow().clone();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1], w.ws, "the exams folder");
    for p in &shown {
        assert!(p.is_dir(), "{} is a folder", p.display());
    }
}

/// D3: Settings shows the exams folder, changes it in the folder dialog and remembers it.
#[test]
fn settings_change_opens_another_exams_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fall = exams_folder(&dir.path().join("fall"), &[&QUIZ_1]);
    let spring = exams_folder(&dir.path().join("spring"), &[&QUIZ_9]);
    let (h, _held, host) = window_on(&fall);
    let ui = &h.ui;
    click(ui, &tab(ui, "Settings"));
    assert!(ui.global::<App>().get_workspace().ends_with("fall"));
    host.will_pick(&[&spring]);
    press(ui, "Change the exams folder…");
    assert_eq!(courses(ui), ["CS 299"]);
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Exams);
    assert_eq!(
        last_saved(&host).workspace.as_deref(),
        Some(spring.as_path())
    );
}

/// D4: an empty folder offers what to do: New exam, another folder (the folder dialog), and
/// the Remark import; the folder button needs the dialog.
#[test]
fn empty_workspace_shows_no_exams_yet_with_three_actions() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    one(ui, "No exams in this folder yet");
    assert!(
        by_label(ui, "Open another folder…").is_empty(),
        "no folder dialog, no button"
    );
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam…");
    let ex = ui.global::<ExamsVM>();
    assert!(ex.get_new_open(), "the form opens");
    press(ui, "Cancel");
    assert!(!ex.get_new_open());
    host.will_cancel();
    press(ui, "Open another folder…");
    assert_eq!(host.last_request().target, PickTarget::Workspace);
    one(ui, "No exams in this folder yet");
    press(ui, "Import from Remark export…");
    assert!(ex.get_new_open());
    assert!(ui.global::<RemarkVM>().get_new_panel(), "the Remark import");
}

/// D4: a status filter that hides every exam says so and shows them all again in one click.
#[test]
fn filter_hiding_every_row_offers_show_all() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, _held, _host) = window_on(&ws);
    let ui = &h.ui;
    press(ui, "Released, 0");
    assert!(courses(ui).is_empty());
    one(ui, "No exams with this status");
    press(ui, "Show all exams");
    assert_eq!(courses(ui), ["CS 213"]);
    assert_eq!(ui.global::<ExamsVM>().get_filter(), 0);
    assert!(by_label(ui, "No exams with this status").is_empty());
}

/// Every control for folders and settings sits inside the smallest window, in both themes.
#[test]
fn folder_and_settings_controls_fit_the_smallest_window() {
    let dir = tempfile::tempdir().expect("temp dir");
    // a long folder name: Settings shows it on two lines
    let long = dir
        .path()
        .join("Final exams of the fall term, Computer Science");
    let ws = exams_folder(&long, &[&QUIZ_1]);
    let (h, _held, _host) = window_on(&ws);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let inside = |label: &str, what: &str| {
        let e = one(ui, label);
        let (p, sz) = (e.absolute_position(), e.size());
        assert!(
            p.x >= 0.0 && p.y >= 0.0 && p.x + sz.width <= 1280.5 && p.y + sz.height <= 800.5,
            "{what}: {label:?} at ({}, {}) size {}x{} is outside the window",
            p.x,
            p.y,
            sz.width,
            sz.height
        );
    };
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        let what = format!("dark {dark}");
        click(ui, &tab(ui, "Exams"));
        for l in [
            "Open folder…",
            "Refresh the exams list (F5)",
            "New exam",
            "Open exam folder",
            "Scan and recognise",
        ] {
            inside(l, &what);
        }
        click(ui, &tab(ui, "Results"));
        // release is blocked (the key is inferred): the long reason never widens the screen
        assert!(ui.global::<ResultsVM>().get_release_blocked());
        let toggle = if dark { "Light theme" } else { "Dark theme" };
        for l in ["Release results…", toggle] {
            inside(l, &what);
        }
        press(ui, "Export…");
        for l in ["Write all exports", "Open exports folder", "Close"] {
            inside(l, &what);
        }
        press(ui, "Close");
        click(ui, &tab(ui, "Settings"));
        let reveal = ui.global::<App>().get_reveal_label().to_string();
        for l in [
            "Your name in the audit trail",
            "Save your name",
            "Change the exams folder…",
            reveal.as_str(),
            "Show blank-bubble overlays",
        ] {
            inside(l, &what);
        }
        for bar in ElementHandle::find_by_element_type_name(ui, "StatusBar") {
            let (p, sz) = (bar.absolute_position(), bar.size());
            assert!(
                p.y + sz.height <= 800.5,
                "{what}: Settings' status bar ends at {} past the window",
                p.y + sz.height
            );
        }
    }
}

// ------------------------------------------------------------------ settings (D9)

/// Clicks into the field labelled `label`, selects what it holds and types `text` over it.
fn retype(ui: &AppWindow, label: &str, text: &str) {
    click(ui, &one(ui, label));
    chord(ui, Key::Control, "a");
    special(ui, Key::Backspace);
    type_text(ui, text);
}

/// D9: the name given in Settings is recorded on the next decision (in the exam open now and
/// in exams opened later) and kept for the next start.
#[test]
fn operator_name_is_recorded_on_the_next_decision() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let other = exams_folder(&dir.path().join("other"), &[&QUIZ_9]);
    let (h, _held, host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Settings"));
    let app = ui.global::<App>();
    assert_eq!(app.get_operator_name(), "a11", "the name recorded now");
    retype(ui, "Your name in the audit trail", "  Huda   A ");
    press(ui, "Save your name");
    let msg = message(ui);
    assert!(
        msg.contains("Huda A") && msg.contains("from now on"),
        "{msg}"
    );
    assert_eq!(app.get_user_name(), "Huda A");
    assert_eq!(app.get_user_initials(), "HA");
    assert_eq!(app.get_operator_name(), "Huda A", "as recorded");
    assert_eq!(last_saved(&host).operator.as_deref(), Some("Huda A"));

    click(ui, &tab_of(ui, "Review"));
    press(ui, "Accept suggestion");
    assert_eq!(last_actor(&h, &h.exam), "principal:Huda A");

    // an exam opened after the change records it too; Enter in the field saves as well
    host.will_pick(&[&other]);
    click(ui, &tab(ui, "Exams"));
    press(ui, "Open folder…");
    let exam = h.app.borrow().default_exam().expect("an exam");
    click(ui, &tab_of(ui, "Review"));
    press(ui, "Accept suggestion");
    assert_eq!(last_actor(&h, &exam), "principal:Huda A");
    click(ui, &tab(ui, "Settings"));
    retype(ui, "Your name in the audit trail", "هدى");
    special(ui, Key::Return);
    assert_eq!(app.get_user_name(), "هدى");
    assert_eq!(last_saved(&host).operator.as_deref(), Some("هدى"));
}

/// D9: a name that is empty, too long or holds a colon is refused and changes nothing.
#[test]
fn bad_operator_names_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, _held, host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Settings"));
    for (name, why) in [
        ("Huda: A", "colon"),
        ("   ", "type the name"),
        (&"x".repeat(65), "at most 64 characters"),
    ] {
        retype(ui, "Your name in the audit trail", name);
        press(ui, "Save your name");
        let msg = message(ui);
        assert!(
            msg.starts_with("Your name: ") && msg.contains(why),
            "{name:?}: {msg}"
        );
        assert_eq!(
            ui.global::<App>().get_message_tone(),
            shell_slint::ui::Tone::Warning
        );
        assert_eq!(ui.global::<App>().get_user_name(), "a11", "{name:?}");
    }
    assert!(host.saved.borrow().is_empty(), "nothing saved");
    click(ui, &tab_of(ui, "Review"));
    press(ui, "Accept suggestion");
    assert_eq!(last_actor(&h, &h.exam), "principal:a11");
}

/// D9: the theme, reduced motion, one-key review and the blank-bubble overlays are saved as
/// they are changed, and the next start applies them.
#[test]
fn theme_choice_is_saved() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    let theme = ui.global::<Theme>();
    let app = ui.global::<App>();
    click(ui, &tab(ui, "Settings"));
    press(ui, "Dark theme setting");
    assert!(theme.get_dark());
    assert_eq!(last_saved(&host).theme.as_deref(), Some("dark"));
    // the top bar's switch saves too
    press(ui, "Light theme");
    assert!(!theme.get_dark());
    assert_eq!(last_saved(&host).theme.as_deref(), Some("light"));
    press(ui, "Dark theme");
    assert_eq!(last_saved(&host).theme.as_deref(), Some("dark"));

    assert!(theme.get_reduce_motion(), "the harness reduces motion");
    press(ui, "Reduce motion");
    assert!(!theme.get_reduce_motion());
    assert!(!last_saved(&host).reduce_motion);
    press(ui, "Reduce motion");
    assert!(last_saved(&host).reduce_motion);
    press(ui, "Single-key shortcuts");
    assert!(!app.get_single_key());
    assert!(!last_saved(&host).single_key);
    press(ui, "Show blank-bubble overlays");
    assert!(app.get_show_all_default());
    let saved = last_saved(&host);
    assert!(saved.show_all_default);
    assert_eq!(
        (
            saved.theme.as_deref(),
            saved.reduce_motion,
            saved.single_key
        ),
        (Some("dark"), true, false)
    );

    // the next start: the window as it opens, given these preferences
    theme.set_dark(false);
    theme.set_reduce_motion(false);
    app.set_single_key(true);
    app.set_show_all_default(false);
    w.h.shell.set_host(FakeHost::new().host(saved));
    assert!(theme.get_dark() && theme.get_reduce_motion());
    assert!(!app.get_single_key() && app.get_show_all_default());
    w.h.shell.set_host(FakeHost::new().host(Prefs::default()));
    assert!(theme.get_dark(), "no theme saved: the window keeps its own");
    assert!(app.get_single_key());
}
