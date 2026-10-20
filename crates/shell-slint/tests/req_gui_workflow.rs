//! The desktop workflow without the command line (G-3): file dialogs on every path field. The
//! window gets what the binary supplies from the operating system through a fake host (it
//! records each dialog asked for and answers from a queue), and is driven as a user does:
//! real pointer clicks and typed keys, on the synthetic exam (no real data).
//!
//! Each test runs on its own thread with its own Slint testing platform.

mod support;

use shell_slint::app_backend::AppBackend;
use shell_slint::backend::Backend;
use shell_slint::host::{PickKind, PickTarget, Prefs};
use shell_slint::ui::{App, AppWindow, ExamsVM, RemarkVM, ScanVM, SetupVM};
use slint::ComponentHandle;
use slint::platform::Key;
use std::path::{Path, PathBuf};
use support::a11::{chord, click, one, tab, type_text, with_prefix};
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
