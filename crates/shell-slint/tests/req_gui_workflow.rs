//! The desktop workflow without the command line (G-3): file dialogs on every path field,
//! opening another exams folder, the file manager, and the settings kept between starts. The
//! window gets what the binary supplies from the operating system through a fake host (it
//! records each dialog asked for and answers from a queue, each folder shown and each save of
//! the preferences), and is driven as a user does: real pointer clicks and typed keys, on
//! synthetic exams (no real data).
//!
//! Each test runs on its own thread with its own Slint testing platform.

mod support;

use i_slint_backend_testing::{AccessibleLiveness, AccessibleRole, ElementHandle};
use omr_app::ExamProject;
use omr_app::define::Sheet;
use omr_app::propose::{Answer, Context, Given, propose};
use shell_slint::app_backend::AppBackend;
use shell_slint::backend::{Backend, ExamId};
use shell_slint::host::{PickKind, PickTarget, Prefs};
use shell_slint::screenshot;
use shell_slint::ui::{
    App, AppWindow, ExamsVM, RemarkVM, ResultsVM, ReviewVM, ScanVM, SetupVM, Theme,
};
use slint::platform::Key;
use slint::{ComponentHandle, Model};
use std::path::{Path, PathBuf};
use support::a11::{by_label, chord, click, one, special, tab, type_text, with_prefix};
use support::harness::{
    FakeHost, Harness, Held, grade, harness, harness_in, held_spawner, run_held, run_survey,
    settle_card,
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

/// The card's answer to "no answer key" that grades without one (these exams have none).
const PROVISIONAL: &str = "Grade with a provisional key (cannot be released)";

/// Types `fields` (label, text) on the open New-exam card, lets it check the scans, answers the
/// missing key with the provisional one and grades: the run is held.
fn grade_with_card(h: &Harness, held: &Held, fields: &[(&str, &str)]) {
    let ui = &h.ui;
    for (label, text) in fields {
        type_into(ui, label, text);
    }
    settle_card(h, held);
    press(ui, PROVISIONAL);
    press(ui, "Grade exam");
}

/// D1: without file dialogs (headless tests) no Browse button exists and typed paths grade as
/// before; given a host, every path field of the New-exam form has its buttons.
#[test]
fn no_host_hides_browse_buttons() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    assert!(ui.global::<ExamsVM>().get_new_open(), "the form opens");
    assert!(!ui.global::<App>().get_can_browse());
    assert_eq!(browse_buttons(ui), Vec::<String>::new());
    grade_with_card(
        &w.h,
        &w.held,
        &[("Scan files or folders", w.scans.to_str().unwrap())],
    );
    let msg = message(ui);
    assert!(msg.contains("Grading 6 scan files"), "{msg}");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    let d = w.h.app.borrow().dashboard();
    assert_eq!(d.exams.len(), 1);
    assert_eq!(d.exams[0].sheets_scanned, 6);

    // the desktop app's window: Browse… beside each path, Folder… for the scans
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    assert!(ui.global::<App>().get_can_browse());
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    let shown = [
        "Browse for Answer key file",
        "Browse for CLO mapping file",
        "Browse for Key sheets",
        "Browse for Roster file",
        "Browse for Scan files or folders",
        "Choose a folder for Scan files or folders",
    ];
    assert_eq!(browse_buttons(ui), texts(&shown));
    // the sheet and the exam's folder under Advanced options
    press(ui, "Advanced options");
    let mut all = texts(&shown);
    all.extend(texts(&[
        "Browse for Output folder",
        "Browse for Sheet template or spec file",
    ]));
    all.sort();
    assert_eq!(browse_buttons(ui), all);
    // each button says which dialog it opens: a folder's, or a file's
    for b in &all {
        let said = one(ui, b).accessible_description().unwrap_or_default();
        press(ui, b);
        let folder = host.last_request().kind == PickKind::Folder;
        let want = if folder {
            "Opens the folder dialog; the folder you choose fills the field"
        } else {
            "Opens the file dialog; what you choose fills the field"
        };
        assert_eq!(said, want, "{b}");
    }
    assert_eq!(
        one(ui, "Browse for Output folder").accessible_description(),
        Some("Opens the folder dialog; the folder you choose fills the field".into())
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
    // the picked files are checked at once, exactly as picked
    run_survey(&w.h, &w.held);
    assert_eq!(ui.global::<ExamsVM>().get_survey_state(), 2);
    let scans = one(ui, "Scans: 3 files — typed (3 student sheets)");
    assert_eq!(scans.accessible_role(), Some(AccessibleRole::Text));

    grade_with_card(&w.h, &w.held, &[("Number of questions", "30")]);
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
    grade_with_card(&w.h, &w.held, &[("Number of questions", "30")]);
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
    press(ui, "Advanced options");
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
    assert!(
        one(ui, "Browse for Report folder")
            .accessible_description()
            .is_some_and(|d| d.starts_with("Opens the folder dialog")),
        "a folder's dialog"
    );
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

/// A folder the window proposes below the home folder reads `~`, as the status bar writes it:
/// the parallel run's report folder (read back as the same folder).
#[test]
fn the_proposed_report_folder_reads_a_tilde() {
    let Some(home) = std::env::home_dir() else {
        return;
    };
    let dir = tempfile::Builder::new()
        .prefix(".mizan-test-")
        .tempdir_in(&home)
        .expect("temp dir in the home folder");
    let built = synth::build(&MIDTERM, dir.path());
    let books = remark::write_books_with(&MIDTERM, &dir.path().join("remark"), true);
    let app = AppBackend::project(&built.exam_dir, "principal:a11-instructor").expect("open");
    let h = harness_in(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 2400));
    let host = FakeHost::new();
    h.shell.set_host(host.host(Prefs::default()));
    click(ui, &tab(ui, "Results"));
    press(ui, "Compare with Remark export…");
    let picked: Vec<&Path> = books.iter().map(PathBuf::as_path).collect();
    host.will_pick(&picked);
    press(ui, "Browse for Remark workbooks to compare");
    press(ui, "Compare");
    let rm = ui.global::<RemarkVM>();
    assert!(rm.get_cmp_open(), "{}", message(ui));
    let sep = std::path::MAIN_SEPARATOR;
    let report = built.exam_dir.join("parallel-run-remark");
    let rel = report.strip_prefix(&home).expect("below home").display();
    assert_eq!(rm.get_save_path(), format!("~{sep}{rel}"));
    assert_eq!(
        shell_slint::bind::one_path(&rm.get_save_path()),
        Some(report.clone())
    );
}

/// Turns the mouse wheel over the element labelled `label` by `dy` logical pixels (negative:
/// the content moves up, as scrolling down does).
fn wheel(ui: &AppWindow, label: &str, dy: f32) {
    let e = one(ui, label);
    let (p, sz) = (e.absolute_position(), e.size());
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(p.x + sz.width / 2.0, p.y + sz.height / 2.0),
            delta_x: 0.0,
            delta_y: dy,
        });
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
        press(ui, "Advanced options");
        // the card scrolls to them in the smallest window
        wheel(ui, "Advanced options", -2000.0);
        buttons_inside(ui, &what("New exam, advanced options"));
        wheel(ui, "Output folder", 2000.0);
        press(ui, "Advanced options");
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
    grade_with_card(
        &w.h,
        &w.held,
        &[("Scan files or folders", w.scans.to_str().unwrap())],
    );
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
    grade_with_card(
        &h,
        &held,
        &[
            ("Scan files or folders", scans.to_str().unwrap()),
            ("Course code", "CS 299"),
        ],
    );
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
    grade_with_card(
        &w.h,
        &w.held,
        &[("Scan files or folders", w.scans.to_str().unwrap())],
    );
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

/// D3: the Settings keymap lists the keys of the exams folder, Ctrl+O and F5, where they work,
/// and the About card speaks of the exams folder, as the window does everywhere else.
#[test]
fn settings_keymap_lists_the_folder_keys() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    click(ui, &tab(ui, "Settings"));
    one(ui, "Exams: open folder · refresh");
    let note = ui.global::<App>().get_data_note().to_string();
    assert!(
        note.contains("the exams in this folder") && !note.contains("workspace"),
        "{note}"
    );
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
            "Write exports after grading",
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

// ------------------------------------------------------------------ the New-exam card (D5)

/// The value of the card's row `label` ("Questions: 30 — the sheets (…)"), as read aloud (a
/// question or note on the field starts the same, without the source).
fn card_row(ui: &AppWindow, label: &str) -> String {
    let rows: Vec<String> = with_prefix(ui, &format!("{label}: "))
        .into_iter()
        .map(|(l, _)| l)
        .filter(|l| l.contains(" — "))
        .collect();
    assert_eq!(rows.len(), 1, "one card row {label}: {rows:?}");
    rows[0].clone()
}

/// The card's row `label` as it reads, from what the card shows: a row scrolled out of the
/// card's view (below open options, or with a run's notice on show) is not in the tree.
fn card_value(ui: &AppWindow, label: &str) -> String {
    let rows = ui.global::<ExamsVM>().get_card_values();
    let row = rows.iter().find(|v| v.label == label);
    row.map(|v| v.a11y.to_string()).unwrap_or_default()
}

/// True when the button labelled `label` can be pressed.
fn enabled(ui: &AppWindow, label: &str) -> bool {
    one(ui, label).accessible_enabled() != Some(false)
}

/// D5: the scans picked are checked at once in the background, with progress and a way to stop;
/// when the check ends every value of the card shows its source and evidence, the question
/// that stops grading carries its one-click answers, and the notes stay folded until asked for.
#[test]
fn the_survey_runs_at_once_and_fills_the_card() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    assert!(!enabled(ui, "Grade exam"), "nothing to grade yet");
    host.will_pick(&[&w.scans]);
    press(ui, "Choose a folder for Scan files or folders");
    // checking: no pause needed, a picked folder is checked at once
    assert!(w.h.app.borrow().surveying());
    assert_eq!(ex.get_survey_state(), 1);
    one(ui, "Stop checking");
    assert!(!enabled(ui, "Grade exam"));
    let why = one(ui, "Grade exam")
        .accessible_description()
        .unwrap_or_default()
        .to_string();
    assert!(why.starts_with("Checking the scans"), "{why}");
    assert!(
        card_row(ui, "Scans").contains("not checked yet"),
        "{}",
        card_row(ui, "Scans")
    );

    run_survey(&w.h, &w.held);
    assert_eq!(ex.get_survey_state(), 2);
    assert!(by_label(ui, "Stop checking").is_empty());
    assert!(
        ex.get_survey_text().starts_with("Checked 6 pages in "),
        "{}",
        ex.get_survey_text()
    );
    for (label, value) in [
        ("Scans", "Scans: 6 files — typed (6 student sheets)"),
        ("Questions", "Questions: 30 — the sheets (sheets: rows 1–30"),
        ("Options", "Options: 5 — the sheet"),
        ("Cancel mode", "Cancel mode: Erase to change — the sheet"),
        ("Student ID", "Student ID: IAU student number — the sheet"),
        ("Answer key", "Answer key: none — default"),
    ] {
        let row = card_row(ui, label);
        assert!(row.starts_with(value), "{row}");
    }
    // the blank fields show what they will grade
    assert_eq!(ex.get_h_questions().proposed, "30 · the sheets");
    // the open question, its answers, and Grade exam waiting with it
    let asks = ex.get_card_asks();
    assert_eq!(asks.row_count(), 1);
    let ask = asks.row_data(0).unwrap();
    assert_eq!(ask.label, "Answer key");
    for choice in [
        "Choose a key file…",
        "Import from Remark…",
        "Key sheets…",
        PROVISIONAL,
    ] {
        one(ui, choice);
    }
    assert!(!enabled(ui, "Grade exam"));
    // the field's label says what is missing, once
    let block = ex.get_new_block().to_string();
    assert!(
        block.starts_with("Answer key: choose the key file"),
        "{block}"
    );
    assert!(!block.contains("key: no answer key"), "{block}");
    assert!(
        block.ends_with("(the results cannot be released until the real key replaces it)"),
        "{block}"
    );
    // a row is read with the marker shown beside it: the question, a note
    let key = card_row(ui, "Answer key");
    assert!(key.ends_with(", question"), "{key}");
    let noted = ex
        .get_card_values()
        .iter()
        .find(|v| v.state == 1)
        .expect("a value with a note");
    assert!(noted.a11y.ends_with(", note"), "{}", noted.a11y);
    assert!(
        card_row(ui, "Options").ends_with("printed)"),
        "nothing to say"
    );
    // the notes, folded: the toggle is read as it shows, with the count
    let n = ex.get_card_notes().row_count();
    assert!(n > 0);
    assert!(by_label(ui, "Notes").is_empty(), "named as shown");
    let notes = one(ui, &format!("Show notes ({n})"));
    assert_eq!(notes.accessible_expanded(), Some(false));
    let note = ex.get_card_notes().row_data(0).unwrap();
    let note_text = format!("{}: {}", note.label, note.text);
    assert!(by_label(ui, &note_text).is_empty(), "folded");
    click(ui, &notes);
    assert_eq!(
        one(ui, &format!("Hide notes ({n})")).accessible_expanded(),
        Some(true)
    );
    one(ui, &note_text);
}

/// D5: a question blocks Grade exam (clicked or not, nothing is graded); its one-click answer
/// unblocks it. A provisional key is used only after that explicit choice, and the choice is
/// not carried to other scans.
#[test]
fn an_ask_blocks_grade_and_a_choice_unblocks_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert!(w.held.borrow().is_empty(), "the survey ran");
    assert!(!enabled(ui, "Grade exam"));
    press(ui, "Grade exam");
    assert!(!w.h.app.borrow().grading_in_progress());
    assert!(
        w.held.borrow().is_empty(),
        "nothing graded while a question is open"
    );
    assert!(card_row(ui, "Answer key").starts_with("Answer key: none — default"));

    press(ui, PROVISIONAL);
    assert_eq!(ex.get_card_asks().row_count(), 0);
    assert_eq!(ex.get_new_block(), "");
    assert!(enabled(ui, "Grade exam"));
    let key = card_row(ui, "Answer key");
    assert!(
        key.starts_with("Answer key: provisional (inferred from the answers) — your answer"),
        "{key}"
    );

    // other scans: a new survey, and the answer given for the old ones is gone
    let other = synth::write_scans(
        &synth::Spec {
            sheets: 4,
            ..synth::BARE
        },
        &dir.path().join("other scans"),
    );
    click(ui, &one(ui, "Scan files or folders"));
    chord(ui, Key::Control, "a");
    type_text(ui, other.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert!(card_row(ui, "Scans").starts_with("Scans: 4 files — typed (4 student sheets"));
    assert!(card_row(ui, "Answer key").starts_with("Answer key: none — default"));
    assert!(!enabled(ui, "Grade exam"), "asked again for these scans");

    press(ui, PROVISIONAL);
    press(ui, "Grade exam");
    assert!(
        message(ui).contains("Grading 4 scan files"),
        "{}",
        message(ui)
    );
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let key =
        w.h.app
            .borrow()
            .with_project(&exam, |p| p.session.definition.key.clone())
            .expect("project");
    assert_eq!(key, omr_app::exam::KeySource::Inferred);
}

/// D5, the contract: the definition graded is the one the proposal's inputs define, typed
/// values included (one of them under Advanced options), and it records where each value came
/// from.
#[test]
fn the_graded_definition_equals_the_proposals_inputs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    press(ui, "Advanced options");
    for (label, text) in [
        ("Scan files or folders", w.scans.to_str().unwrap()),
        ("Exam title", "Quiz 7"),
        ("Course code", "CS 213"),
        ("Versions", "A, B"),
        ("Term", "1447-2"),
    ] {
        type_into(ui, label, text);
    }
    settle_card(&w.h, &w.held);
    press(ui, PROVISIONAL);
    let folder = card_row(ui, "Folder");
    press(ui, "Grade exam");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);

    // the same proposal made by the core alone
    let mut given = Given::new(
        Sheet::Template("iau-ccsit-v1".into()),
        vec![w.scans.clone()],
    );
    given.typed.title = Some("Quiz 7".into());
    given.typed.course = Some("CS 213".into());
    given.typed.forms = Some("A, B".into());
    given.typed.term = Some("1447-2".into());
    given.answers = vec![Answer::InferKeyProvisionally];
    let survey = omr_app::survey::survey(&given.survey_request(1), &|_| {}).expect("survey");
    // (the exams folder now holds the new exam: the proposal is made as it was before)
    let p = propose(&given, Some(&survey), &Context::new(&w.ws));
    let inputs = p.inputs.expect("no question open");
    let (expected, _) = omr_app::define::define(&inputs).expect("define");
    assert!(folder.contains("cs-213-quiz-7 —"), "{folder}");
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    assert_eq!(
        w.h.app.borrow().exam_folder(&exam),
        Some(w.ws.join("cs-213-quiz-7")),
        "graded into the card's folder"
    );
    let graded =
        w.h.app
            .borrow()
            .with_project(&exam, |p| p.session.definition.clone())
            .expect("project");
    assert_eq!(graded, expected);
    assert_eq!(graded.term, "1447-2");
    assert_eq!(graded.forms, ["A", "B"]);
    let source = |field: &str| {
        graded
            .provenance
            .iter()
            .find(|f| f.field == field)
            .map(|f| (f.value.clone(), f.source.clone()))
    };
    assert_eq!(
        source("questions"),
        Some(("30".into(), "the sheets".into()))
    );
    assert_eq!(source("versions"), Some(("A, B".into(), "typed".into())));
    assert_eq!(source("term"), Some(("1447-2".into(), "typed".into())));
}

/// D5: editing a value proposes the card again once typing pauses, without reading the scans
/// again; other scans start a new survey, and the survey of the old scans is discarded even if
/// it ends after the new one started.
#[test]
fn editing_the_scans_discards_the_old_survey() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert!(card_row(ui, "Scans").starts_with("Scans: 6 files"));

    // a typed title: shown once typing pauses, the scans are not read again
    type_into(ui, "Exam title", "Quiz 7");
    assert!(
        !card_row(ui, "Title").starts_with("Title: Quiz 7"),
        "not on every key"
    );
    support::harness::pause_typing();
    assert!(card_row(ui, "Title").starts_with("Title: Quiz 7 — typed"));
    assert!(!w.h.app.borrow().surveying());
    assert!(w.held.borrow().is_empty(), "no survey started");

    // other scans: their survey starts; the first scans' survey (started again by another
    // change, then left behind) never reaches the card
    let four = synth::write_scans(
        &synth::Spec {
            sheets: 4,
            ..synth::BARE
        },
        &dir.path().join("four"),
    );
    let five = synth::write_scans(
        &synth::Spec {
            sheets: 5,
            ..synth::BARE
        },
        &dir.path().join("five"),
    );
    click(ui, &one(ui, "Scan files or folders"));
    chord(ui, Key::Control, "a");
    type_text(ui, four.to_str().unwrap());
    support::harness::pause_typing();
    assert_eq!(w.held.borrow().len(), 1);
    click(ui, &one(ui, "Scan files or folders"));
    chord(ui, Key::Control, "a");
    type_text(ui, five.to_str().unwrap());
    support::harness::pause_typing();
    assert_eq!(w.held.borrow().len(), 2, "a survey for each stack");
    // the survey of the four sheets ends: dropped
    let stale = w.held.borrow_mut().remove(0);
    std::thread::spawn(stale).join().expect("stale survey");
    w.h.shell.pump();
    assert!(w.h.app.borrow().surveying(), "still checking the five");
    assert_eq!(ex.get_survey_state(), 1);
    assert!(card_row(ui, "Scans").contains("not checked yet"));
    run_survey(&w.h, &w.held);
    assert!(card_row(ui, "Scans").starts_with("Scans: 5 files — typed (5 student sheets"));
    assert!(card_row(ui, "Title").starts_with("Title: Quiz 7 — typed"));
}

/// D5: Advanced options are collapsed, and their fields do not exist, until clicked; the IAU
/// sheet's cancel mode is shown locked with the value it grades, in plain words; second review
/// is chosen from its choices (never typed), and the high-stakes switch reads its state first.
#[test]
fn advanced_options_are_collapsed_until_clicked() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    let fields = [
        "Sheet template or spec file",
        "Output folder",
        "Term",
        "Student ID scheme",
        "Cancel mode",
        "Second review",
        "High-stakes exam",
    ];
    let advanced = one(ui, "Advanced options");
    assert_eq!(advanced.accessible_expanded(), Some(false));
    for f in fields {
        assert!(
            by_label(ui, f).is_empty(),
            "{f} exists before Advanced options"
        );
    }
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    press(ui, "Advanced options");
    assert_eq!(
        one(ui, "Advanced options").accessible_expanded(),
        Some(true)
    );
    for f in fields {
        one(ui, f);
    }
    let cancel = one(ui, "Cancel mode");
    assert_eq!(
        cancel.accessible_enabled(),
        Some(false),
        "the sheet fixes it"
    );
    assert_eq!(
        cancel.accessible_placeholder_text().unwrap_or_default(),
        "Erase to change · the sheet"
    );
    let second = one(ui, "Second review");
    let said = second.accessible_description().unwrap_or_default();
    assert!(
        said.starts_with(
            "No second review · default — confident double marks are scored as wrong without \
             review"
        ),
        "{said}"
    );
    let choices: Vec<(String, Option<bool>)> =
        ["No second review", "Always", "On high-stakes exams"]
            .into_iter()
            .map(|l| {
                let e = one(ui, l);
                assert_eq!(
                    e.accessible_role(),
                    Some(AccessibleRole::RadioButton),
                    "{l}"
                );
                (l.to_string(), e.accessible_checked())
            })
            .collect();
    assert_eq!(
        choices.iter().map(|c| c.1).collect::<Vec<_>>(),
        [Some(true), Some(false), Some(false)],
        "{choices:?}"
    );
    let high = one(ui, "High-stakes exam")
        .accessible_description()
        .unwrap_or_default();
    assert!(
        high.starts_with("off · default — when on, stricter margins"),
        "{high}"
    );
    // a choice clicked is the value graded, as typed, and grading waits on nothing else
    press(ui, "On high-stakes exams");
    assert_eq!(
        one(ui, "On high-stakes exams").accessible_checked(),
        Some(true)
    );
    support::harness::pause_typing();
    // (the row is below the open options, scrolled out of the card's view)
    let row = card_value(ui, "Second review");
    assert!(
        row.starts_with("Second review: On high-stakes exams — typed"),
        "{row}"
    );
    press(ui, PROVISIONAL);
    let block = ui.global::<ExamsVM>().get_new_block();
    assert_eq!(block, "", "nothing asked of the sheet");
    press(ui, "Advanced options");
    for f in fields {
        assert!(by_label(ui, f).is_empty(), "{f} gone once folded");
    }
}

/// D5: a sheet that prints its cancel mode fixes it, whether it is the IAU template or an own
/// sheet's spec file: the field is shown locked with the spec's mode in plain words, as the core
/// says, never left open to type a mode the sheet does not print.
#[test]
fn a_spec_file_that_prints_its_cancel_mode_fixes_the_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let spec = omr_forms::TemplateSpec::standard(
        omr_model::ExamId::new_v7(),
        30,
        5,
        omr_model::CancelMode::CrossCancels,
    );
    let json = serde_json::json!({"template": spec, "exam_key_hex": "6c".repeat(32)});
    let file = dir.path().join("own-sheet.json");
    std::fs::write(&file, serde_json::to_vec(&json).expect("json")).expect("spec file");
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    press(ui, "Advanced options");
    click(ui, &one(ui, "Sheet template or spec file"));
    chord(ui, Key::Control, "a");
    type_text(ui, file.to_str().unwrap());
    settle_card(&w.h, &w.held);
    let cancel = one(ui, "Cancel mode");
    assert_eq!(
        cancel.accessible_enabled(),
        Some(false),
        "the spec fixes it"
    );
    assert_eq!(
        cancel.accessible_placeholder_text().unwrap_or_default(),
        "Cross out to cancel · spec file"
    );
    let row = card_row(ui, "Cancel mode");
    assert!(
        row.starts_with("Cancel mode: Cross out to cancel — spec file"),
        "{row}"
    );
}

/// D5: a one-click answer that gives a field's value holds only while the field keeps the value
/// it was given for: typed again, the field's new value is graded (and checked afresh), never
/// the earlier answer.
#[test]
fn an_answer_holds_only_while_its_field_keeps_its_value() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    press(ui, PROVISIONAL);
    // options: 9 is asked, 5 answered, then 4 typed
    type_into(ui, "Options per question", "9");
    support::harness::pause_typing();
    press(ui, "Use 5");
    assert!(card_row(ui, "Options").starts_with("Options: 5 — your answer"));
    click(ui, &one(ui, "Options per question"));
    chord(ui, Key::Control, "a");
    type_text(ui, "4");
    support::harness::pause_typing();
    let row = card_row(ui, "Options");
    assert!(row.starts_with("Options: 4 — typed"), "{row}");
    assert_eq!(ex.get_new_block(), "", "4 options graded, nothing asked");
    // the count: 25 is asked (the students answer 30 rows), 30 answered, then 20 typed
    type_into(ui, "Number of questions", "25");
    support::harness::pause_typing();
    let use30 = with_prefix(ui, "Use 30");
    let labels: Vec<&String> = use30.iter().map(|(l, _)| l).collect();
    assert_eq!(use30.len(), 1, "{labels:?}");
    click(ui, &use30[0].1);
    assert!(card_row(ui, "Questions").starts_with("Questions: 30 — your answer"));
    click(ui, &one(ui, "Number of questions"));
    chord(ui, Key::Control, "a");
    type_text(ui, "20");
    support::harness::pause_typing();
    let row = card_row(ui, "Questions");
    assert!(row.starts_with("Questions: 20 — typed"), "{row}");
    let asked: Vec<String> = ex
        .get_card_asks()
        .iter()
        .map(|a| a.label.to_string())
        .collect();
    assert_eq!(asked, ["Questions"], "the rows beyond 20 are asked again");
}

/// D5: an exam folder typed as a name (not a whole path) is a folder in the exams folder: the
/// card shows where, the exam is graded there, and it is listed when the folder is read again.
#[test]
fn a_typed_folder_name_grades_into_the_exams_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    press(ui, "Advanced options");
    type_into(ui, "Output folder", "quiz-rel");
    settle_card(&w.h, &w.held);
    press(ui, PROVISIONAL);
    let row = card_row(ui, "Folder");
    let want = w.ws.join("quiz-rel");
    assert!(
        row.starts_with(&format!("Folder: {} — typed", want.display())),
        "{row}"
    );
    press(ui, "Grade exam");
    run_held(&w.h, &w.held);
    assert_eq!(w.h.app.borrow().grading_error(), None);
    assert!(want.join("project.omrproj").is_dir(), "in the exams folder");
    special(ui, Key::F5);
    assert_eq!(ui.global::<ExamsVM>().get_rows().row_count(), 1, "listed");
}

/// D5: the answers given for scans are dropped once those scans are taken away, so other scans
/// are asked afresh; answers given before any scans stay when the scans are given.
#[test]
fn emptying_the_scans_drops_the_answers_given_for_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    // an answer given before any scans: it stays once they are given
    type_into(ui, "Options per question", "9");
    support::harness::pause_typing();
    press(ui, "Use 5");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert!(card_row(ui, "Options").starts_with("Options: 5 — your answer"));
    // the provisional key chosen for these scans
    press(ui, PROVISIONAL);
    assert!(card_row(ui, "Answer key").contains("— your answer"));
    // the scans taken away, then others given
    click(ui, &one(ui, "Scan files or folders"));
    chord(ui, Key::Control, "a");
    special(ui, Key::Backspace);
    settle_card(&w.h, &w.held);
    let other = synth::write_scans(
        &synth::Spec {
            sheets: 4,
            ..synth::BARE
        },
        &dir.path().join("other scans"),
    );
    type_text(ui, other.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert!(card_row(ui, "Scans").starts_with("Scans: 4 files"));
    let key = card_row(ui, "Answer key");
    assert!(key.starts_with("Answer key: none — default"), "{key}");
    one(ui, PROVISIONAL);
    assert!(!enabled(ui, "Grade exam"), "asked again for these scans");
}

/// D5: the card opens while another exam is graded; Grade exam waits and says why, and is
/// available once the run has ended.
#[test]
fn the_card_opens_during_a_run_and_says_why_grade_waits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    grade_with_card(
        &w.h,
        &w.held,
        &[
            ("Scan files or folders", w.scans.to_str().unwrap()),
            ("Exam title", "Quiz 1"),
            ("Course code", "CS 213"),
        ],
    );
    assert!(w.h.app.borrow().grading_in_progress());
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    assert!(ex.get_new_open(), "the card opens during the run");
    assert!(!message(ui).contains("not in the desktop shell"));
    let block = ex.get_new_block().to_string();
    assert!(
        block.starts_with("CS 213 Quiz 1 is being graded") && block.contains("when it finishes"),
        "{block}"
    );
    // the scans are checked meanwhile; Grade still waits for the run
    for (label, text) in [
        ("Scan files or folders", w.scans.to_str().unwrap()),
        ("Exam title", "Quiz 1"),
        ("Course code", "CS 213"),
    ] {
        type_into(ui, label, text);
    }
    settle_card(&w.h, &w.held);
    assert_eq!(ex.get_survey_state(), 2);
    press(ui, PROVISIONAL);
    assert!(!enabled(ui, "Grade exam"));
    assert!(ex.get_new_block().contains("is being graded"));
    // the run ends: Grade exam is available, into another folder than the first exam's
    run_held(&w.h, &w.held);
    assert_eq!(ex.get_new_block(), "", "{}", ex.get_new_block());
    assert!(enabled(ui, "Grade exam"));
    // (the run's notice takes room: the last row is scrolled out of the card's view)
    let folder = card_value(ui, "Folder");
    assert!(folder.contains("cs-213-quiz-1-2 —"), "{folder}");
}

/// D5: the check of the scans can be stopped: nothing is graded until they are checked again,
/// which the card offers as a question; a stopped survey that ends later changes nothing.
#[test]
fn stopping_the_survey_holds_grade_until_checked_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    support::harness::pause_typing();
    assert!(w.h.app.borrow().surveying());
    press(ui, "Stop checking");
    assert!(!w.h.app.borrow().surveying());
    assert_eq!(ex.get_survey_state(), 3);
    assert!(
        message(ui).starts_with("Stopped checking the scans"),
        "{}",
        message(ui)
    );
    let ask = ex.get_card_asks().row_data(0).expect("a question");
    assert!(
        ask.text.starts_with("You stopped checking the scans"),
        "{}",
        ask.text
    );
    assert!(!enabled(ui, "Grade exam"));
    assert!(
        ex.get_new_block()
            .starts_with("Scans: You stopped checking"),
        "{}",
        ex.get_new_block()
    );
    // the stopped survey's worker ends: nothing changes
    let stopped = w.held.borrow_mut().pop().expect("the survey");
    std::thread::spawn(stopped).join().expect("stopped survey");
    w.h.shell.pump();
    assert_eq!(ex.get_survey_state(), 3);

    press(ui, "Check the scans again");
    assert!(w.h.app.borrow().surveying());
    run_survey(&w.h, &w.held);
    assert_eq!(ex.get_survey_state(), 2);
    assert!(card_row(ui, "Scans").starts_with("Scans: 6 files — typed (6 student sheets"));
    assert!(
        ex.get_new_block().starts_with("Answer key"),
        "{}",
        ex.get_new_block()
    );
}

/// D5: a value typed that is no value of the field is a question on that field, said under
/// it, and Grade exam waits; fixed, the question goes.
#[test]
fn an_invalid_value_is_marked_on_its_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let ex = ui.global::<ExamsVM>();
    press(ui, "New exam");
    type_into(ui, "Scan files or folders", w.scans.to_str().unwrap());
    type_into(ui, "Number of questions", "thirty");
    settle_card(&w.h, &w.held);
    press(ui, PROVISIONAL);
    let why = one(ui, "Number of questions")
        .accessible_description()
        .unwrap_or_default()
        .to_string();
    assert!(why.contains("not a question count"), "{why}");
    assert!(
        ex.get_new_block().starts_with("Questions:"),
        "{}",
        ex.get_new_block()
    );
    assert!(!enabled(ui, "Grade exam"));
    // the question's own answer puts the keyboard in the field
    press(ui, "Type the count");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    chord(ui, Key::Control, "a");
    type_text(ui, "30");
    support::harness::pause_typing();
    assert_eq!(ex.get_new_questions(), "30");
    assert_eq!(ex.get_h_questions().error, "");
    assert!(enabled(ui, "Grade exam"));
    assert!(card_row(ui, "Questions").starts_with("Questions: 30 — typed"));
}

// ------------------------------------------------------- after a run: exports, the notice (D6)

/// The New-exam card's fields for CS 213 Quiz 1 graded from `scans`.
fn quiz_1(scans: &Path) -> [(&'static str, &str); 3] {
    [
        ("Scan files or folders", scans.to_str().unwrap()),
        ("Exam title", "Quiz 1"),
        ("Course code", "CS 213"),
    ]
}

/// The completion notice as shown: its title, its text and its buttons, in order.
fn notice(ui: &AppWindow) -> (String, String, Vec<String>) {
    let app = ui.global::<App>();
    assert!(app.get_notice_open(), "the notice is on show");
    let group = one(ui, &app.get_notice_title());
    assert_eq!(
        group.accessible_description().unwrap_or_default(),
        app.get_notice_text()
    );
    // it appears without the keyboard: announced as it appears, at once when the run failed
    let live = if format!("{:?}", app.get_notice_tone()) == "Error" {
        AccessibleLiveness::Assertive
    } else {
        AccessibleLiveness::Polite
    };
    assert_eq!(group.accessible_live_region(), Some(live), "announced");
    let steps = app
        .get_notice_steps()
        .iter()
        .map(|s| s.to_string())
        .collect();
    (
        app.get_notice_title().to_string(),
        app.get_notice_text().to_string(),
        steps,
    )
}

/// Makes the window the smallest it may be (1280 × 800).
fn smallest_window(ui: &AppWindow) {
    window_size(ui, 1280, 800);
}

/// Resizes the window. The screens follow the size on the next event, so the pointer is moved
/// once (over the rail's logo).
fn window_size(ui: &AppWindow, width: u32, height: u32) {
    ui.window()
        .set_size(slint::PhysicalSize::new(width, height));
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(4.0, 4.0),
        });
}

/// Asserts that the control labelled `label` lies inside the smallest window (1280 × 800).
fn fits(ui: &AppWindow, label: &str) {
    for e in by_label(ui, label) {
        let (p, sz) = (e.absolute_position(), e.size());
        assert!(
            p.x >= 0.0 && p.y >= 0.0 && p.x + sz.width <= 1280.5 && p.y + sz.height <= 800.5,
            "{label:?} at ({}, {}) size {}x{} is outside the window",
            p.x,
            p.y,
            sz.width,
            sz.height
        );
    }
    assert!(!by_label(ui, label).is_empty(), "{label:?} is on show");
}

/// The files in `dir`, sorted.
fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("folder")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// The instructor's key file of the synthetic exam `spec`: one column per version.
fn key_file(spec: &synth::Spec, path: &Path) {
    let mut csv = "question,A,B\n".to_string();
    for q in 0..synth::QUESTIONS {
        csv.push_str(&(q + 1).to_string());
        for f in 0..2 {
            csv.push(',');
            csv.push(char::from(b'A' + synth::key(spec, f, q)));
        }
        csv.push('\n');
    }
    std::fs::write(path, csv).expect("key file");
}

/// The open review items of `exam`, as the rail counts them.
fn review_open(h: &Harness, exam: &ExamId) -> u32 {
    let q = h
        .app
        .borrow()
        .review_queue(exam, shell_slint::backend::Lane::Marks);
    q.lanes.iter().map(|l| l.open).sum()
}

/// D6: grading writes the exports on the worker, as `omr grade` does (the same files beside
/// the package), and the notice says where; with an inferred key its next step is replacing
/// the key in Setup, which it opens.
#[test]
fn grading_writes_the_exports_like_omr_grade() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    assert!(
        !ui.global::<App>().get_notice_open(),
        "no notice while it runs"
    );
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let folder = w.h.app.borrow().exam_folder(&exam).expect("its folder");
    let written = files_in(&folder);
    for f in [
        "results-INFERRED.csv",
        "report-INFERRED.html",
        "project.omrproj",
    ] {
        assert!(written.contains(&f.to_string()), "{f} in {written:?}");
    }
    // `omr grade` of the same definition and scans writes the same files
    let def =
        w.h.app
            .borrow()
            .with_project(&exam, |p| p.session.definition.clone())
            .expect("project");
    let cli = dir.path().join("cli");
    let opts = omr_app::GradeOptions {
        threads: 2,
        project_dir: Some(cli.join("project.omrproj")),
        snapshot_utc: None,
        operator: "principal:a11".into(),
        max_pages: None,
        cancel: Default::default(),
    };
    let mut p =
        ExamProject::grade(&def, std::slice::from_ref(&w.scans), &opts, &|_| {}).expect("grading");
    p.export(&cli).expect("exports");
    p.close().expect("close");
    assert_eq!(written, files_in(&cli), "the files `omr grade` writes");

    let (title, text, steps) = notice(ui);
    assert_eq!(title, "CS 213 Quiz 1 is graded");
    assert!(text.starts_with("6 booklets · "), "{text}");
    // the folder, its last folders when the path is long
    let sep = std::path::MAIN_SEPARATOR;
    assert!(
        text.contains("exports in ") && text.contains(&format!("exams{sep}cs-213-quiz-1 · ")),
        "{text}"
    );
    assert!(
        text.contains("provisional until you replace the key"),
        "{text}"
    );
    let n = review_open(&w.h, &exam);
    let mut expected = Vec::new();
    if n > 0 {
        expected.push(format!("Review {n} items"));
    }
    expected.extend(texts(&["Replace the key in Setup", "Open exports folder"]));
    assert_eq!(steps, expected);
    // the notice is on every screen until a step leads elsewhere
    click(ui, &tab(ui, "Settings"));
    assert!(ui.global::<App>().get_notice_open());
    press(ui, "Replace the key in Setup");
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Setup);
    assert!(ui.global::<SetupVM>().get_key_open(), "the key bar is open");
    assert!(!ui.global::<App>().get_notice_open(), "the notice went");
}

/// D6: the Scan log names the project a run saved by its last folders, as the status bar does,
/// not by a path longer than the log's column.
#[test]
fn the_scan_log_names_the_saved_project_by_its_last_folders() {
    let dir = tempfile::tempdir().expect("temp dir");
    let deep = dir
        .path()
        .join("exams of the first term 1448, computer science department");
    let w = workspace(&deep);
    let ui = &w.h.ui;
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let batch = w.h.app.borrow().batch_progress(&exam).expect("its batch");
    let saved = batch
        .activity
        .iter()
        .find(|l| l.contains(" · saved "))
        .expect("the saved line")
        .clone();
    let sep = std::path::MAIN_SEPARATOR;
    assert!(
        saved.ends_with(&format!(
            "saved …{sep}exams{sep}cs-213-quiz-1{sep}project.omrproj"
        )),
        "{saved}"
    );
}

/// D6: a keyed exam's notice leads to its review items and its results, and shows the exports
/// folder (the notice stays for that one); × dismisses it.
#[test]
fn notice_offers_review_results_and_the_exports_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    let key = dir.path().join("key.csv");
    key_file(
        &synth::Spec {
            sheets: 6,
            ..synth::BARE
        },
        &key,
    );
    press(ui, "New exam");
    for (label, text) in quiz_1(&w.scans) {
        type_into(ui, label, text);
    }
    type_into(ui, "Answer key file", key.to_str().unwrap());
    settle_card(&w.h, &w.held);
    assert_eq!(ui.global::<ExamsVM>().get_new_block(), "");
    press(ui, "Grade exam");
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let folder = w.h.app.borrow().exam_folder(&exam).expect("its folder");
    assert!(
        folder.join("results.csv").is_file(),
        "{:?}",
        files_in(&folder)
    );
    let n = review_open(&w.h, &exam);
    assert!(n > 0, "the synthetic sheets leave items to review");
    let (title, text, steps) = notice(ui);
    assert_eq!(title, "CS 213 Quiz 1 is graded");
    assert!(text.contains(&format!("{n} items to review")), "{text}");
    assert!(!text.contains("provisional"), "{text}");
    assert_eq!(
        steps,
        [
            format!("Review {n} items"),
            "Open results".into(),
            "Open exports folder".into()
        ]
    );
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Scan);
    // the notice and its buttons fit the smallest window, above the status bar
    smallest_window(ui);
    for l in steps
        .iter()
        .map(String::as_str)
        .chain(["Dismiss the notice"])
    {
        fits(ui, l);
    }
    let group = one(ui, "CS 213 Quiz 1 is graded");
    assert!(group.absolute_position().y + group.size().height <= 800.0 - 28.0);

    // the folder is shown, and the notice stays
    click(
        ui,
        &by_label(ui, "Open exports folder")
            .into_iter()
            .next()
            .expect("the notice's button"),
    );
    assert_eq!(host.revealed.borrow().clone(), vec![folder.clone()]);
    assert!(ui.global::<App>().get_notice_open());
    press(ui, &format!("Review {n} items"));
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Review);
    assert!(!ui.global::<App>().get_notice_open());

    // another run's notice, dismissed with ×
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    grade_with_card(
        &w.h,
        &w.held,
        &[
            ("Scan files or folders", w.scans.to_str().unwrap()),
            ("Exam title", "Quiz 2"),
            ("Course code", "CS 213"),
        ],
    );
    run_held(&w.h, &w.held);
    let (title, ..) = notice(ui);
    assert_eq!(title, "CS 213 Quiz 2 is graded");
    press(ui, "Dismiss the notice");
    assert!(!ui.global::<App>().get_notice_open());
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Scan);
}

/// D6: a key read from key sheets waits for the instructor's confirmation: nothing is exported
/// yet, the notice and the log say why, and its one step is confirming the key in Setup.
#[test]
fn held_key_notice_points_to_setup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let keys = synth::write_key_sheets(&spec, &dir.path().join("keys"), false);
    press(ui, "New exam");
    for (label, text) in quiz_1(&w.scans) {
        type_into(ui, label, text);
    }
    type_into(
        ui,
        "Key sheets",
        &format!("{};{}", keys[0].display(), keys[1].display()),
    );
    settle_card(&w.h, &w.held);
    assert_eq!(ui.global::<ExamsVM>().get_new_block(), "");
    press(ui, "Grade exam");
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let folder = w.h.app.borrow().exam_folder(&exam).expect("its folder");
    assert_eq!(files_in(&folder), ["project.omrproj"], "nothing exported");
    let (title, text, steps) = notice(ui);
    assert_eq!(title, "CS 213 Quiz 1 is read");
    assert!(
        text.contains("waits for your confirmation in Setup")
            && text.contains("the exports wait for it too"),
        "{text}"
    );
    assert_eq!(steps, ["Confirm the key in Setup"]);
    let log: Vec<String> = ui
        .global::<ScanVM>()
        .get_activity()
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert!(
        log.iter()
            .any(|l| l.contains("Exports wait for the key confirmation in Setup")),
        "{log:?}"
    );
    press(ui, "Confirm the key in Setup");
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Setup);
    assert!(ui.global::<SetupVM>().get_ks_open(), "on the key summary");
    one(ui, "Confirm every version");
}

/// D6: with "Write exports after grading" switched off in Settings (saved), a run writes no
/// export and its notice offers no exports folder.
#[test]
fn auto_export_off_writes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let host = FakeHost::new();
    w.h.shell.set_host(host.host(Prefs::default()));
    click(ui, &tab(ui, "Settings"));
    let switch = one(ui, "Write exports after grading");
    assert_eq!(switch.accessible_checked(), Some(true), "on by default");
    click(ui, &switch);
    assert_eq!(
        one(ui, "Write exports after grading").accessible_checked(),
        Some(false)
    );
    assert!(!last_saved(&host).auto_export, "saved for the next start");
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the new exam");
    let folder = w.h.app.borrow().exam_folder(&exam).expect("its folder");
    assert_eq!(files_in(&folder), ["project.omrproj"]);
    let (_, text, steps) = notice(ui);
    assert!(!text.contains("exports"), "{text}");
    assert!(
        !steps.contains(&"Open exports folder".to_string()),
        "{steps:?}"
    );

    // the next start, with the switch saved off: the window as it opens writes none either
    let saved = last_saved(&host);
    w.h.shell.set_host(FakeHost::new().host(Prefs::default()));
    assert!(ui.global::<App>().get_auto_export());
    w.h.shell.set_host(FakeHost::new().host(saved));
    assert!(!ui.global::<App>().get_auto_export());
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    grade_with_card(
        &w.h,
        &w.held,
        &[
            ("Scan files or folders", w.scans.to_str().unwrap()),
            ("Exam title", "Quiz 2"),
            ("Course code", "CS 213"),
        ],
    );
    run_held(&w.h, &w.held);
    let exam = w.h.app.borrow().default_exam().expect("the second exam");
    let folder = w.h.app.borrow().exam_folder(&exam).expect("its folder");
    assert!(folder.ends_with("cs-213-quiz-2"), "{}", folder.display());
    assert_eq!(files_in(&folder), ["project.omrproj"]);
}

/// D6: opening New exam takes down the last run's notice: at 1280 × 800 it lies over the
/// card's Grade exam, which a click would then miss.
#[test]
fn opening_new_exam_takes_down_the_last_runs_notice() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    smallest_window(ui);
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    run_held(&w.h, &w.held);
    assert!(app.get_notice_open());
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    assert!(!app.get_notice_open(), "the card has the window's foot");
    grade_with_card(
        &w.h,
        &w.held,
        &[
            ("Scan files or folders", w.scans.to_str().unwrap()),
            ("Exam title", "Quiz 2"),
            ("Course code", "CS 213"),
        ],
    );
    assert!(message(ui).starts_with("Grading"), "{}", message(ui));
    assert!(w.h.app.borrow().grading_in_progress());
}

/// True when the two elements share no point (one lies over the other otherwise).
fn apart(a: &ElementHandle, b: &ElementHandle) -> bool {
    let (pa, sa, pb, sb) = (
        a.absolute_position(),
        a.size(),
        b.absolute_position(),
        b.size(),
    );
    pa.x + sa.width <= pb.x
        || pb.x + sb.width <= pa.x
        || pa.y + sa.height <= pb.y
        || pb.y + sb.height <= pa.y
}

/// D6: the notice lies over no control: every screen gives up its height (the exams drawer's
/// buttons, a card opened while a run went on), and grading from that card takes the earlier
/// run's notice down (its Edit and grade again would fill the card of this run).
#[test]
fn the_notice_lies_over_no_control() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    smallest_window(ui);
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    // the next exam's card, opened while Quiz 1 is graded
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    for (label, text) in [
        ("Scan files or folders", w.scans.to_str().unwrap()),
        ("Exam title", "Quiz 2"),
        ("Course code", "CS 213"),
    ] {
        type_into(ui, label, text);
    }
    settle_card(&w.h, &w.held);
    press(ui, PROVISIONAL);
    run_held(&w.h, &w.held);
    assert!(app.get_notice_open(), "Quiz 1 is graded");
    let over = one(ui, &app.get_notice_title());
    assert!(
        apart(&one(ui, "Grade exam"), &over),
        "Grade exam is under the notice"
    );
    press(ui, "Grade exam");
    assert!(message(ui).starts_with("Grading"), "{}", message(ui));
    assert!(
        !app.get_notice_open(),
        "the earlier run's notice is done with"
    );
    run_held(&w.h, &w.held);
    // the drawer of the exam graded, with the notice of its run
    click(ui, &tab(ui, "Exams"));
    assert!(ui.global::<ExamsVM>().get_drawer_open());
    let over = one(ui, &app.get_notice_title());
    for l in ["Scan and recognise", "Open results"] {
        assert!(apart(&one(ui, l), &over), "{l} is under the notice");
    }
}

/// D6: the notice from the keyboard: Escape dismisses it, and once it is dismissed with a click
/// on × the screen keys still work (the button that had the keyboard went with it).
#[test]
fn the_notice_is_dismissed_with_escape_and_the_keys_keep_working() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    run_held(&w.h, &w.held);
    assert!(app.get_notice_open());
    assert_eq!(app.get_page(), shell_slint::ui::Page::Scan);
    special(ui, Key::Escape);
    assert!(!app.get_notice_open(), "Escape dismisses it");
    chord(ui, Key::Control, "5");
    assert_eq!(app.get_page(), shell_slint::ui::Page::Results);
    // another run, its notice dismissed with the mouse
    click(ui, &tab(ui, "Exams"));
    press(ui, "New exam");
    grade_with_card(
        &w.h,
        &w.held,
        &[
            ("Scan files or folders", w.scans.to_str().unwrap()),
            ("Exam title", "Quiz 2"),
            ("Course code", "CS 213"),
        ],
    );
    run_held(&w.h, &w.held);
    assert_eq!(app.get_page(), shell_slint::ui::Page::Scan);
    press(ui, "Dismiss the notice");
    assert!(!app.get_notice_open());
    chord(ui, Key::Control, "5");
    assert_eq!(
        app.get_page(),
        shell_slint::ui::Page::Results,
        "the keys still work"
    );
}

/// D6: Review opens on a lane with open items: a lane that has none while others do (cleared,
/// or chosen) says where the items are, and Review shown again from another screen (the rail,
/// the notice's Review N items) opens a lane with items, never "Nothing to review".
#[test]
fn review_opens_on_a_lane_with_items() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, _held, _host) = window_on(&ws);
    let ui = &h.ui;
    let rv = ui.global::<ReviewVM>();
    click(ui, &tab_of(ui, "Review"));
    let empty = rv
        .get_lanes()
        .iter()
        .find(|l| l.count == 0)
        .expect("a lane with nothing open");
    press(ui, &format!("{}, 0", empty.label));
    assert_eq!(rv.get_queue().row_count(), 0);
    let said = rv.get_empty_text().to_string();
    assert!(said.contains("; still open: "), "{said}");
    click(ui, &tab(ui, "Results"));
    click(ui, &tab_of(ui, "Review"));
    let shown = rv
        .get_lanes()
        .iter()
        .find(|l| l.selected)
        .expect("a lane shown");
    assert!(shown.count > 0, "{} has items", shown.label);
    assert!(rv.get_queue().row_count() > 0);
    assert!(by_label(ui, "Nothing to review").is_empty());
}

/// D6: exports written before a change are said to be older than the exam: once a decision
/// follows the run that wrote them, its notice and Results' Export… say so, until Write all
/// exports writes them again.
#[test]
fn exports_older_than_a_later_decision_are_said_to_be() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let roster = dir.path().join("roster.csv");
    roster_file(&QUIZ_1, &roster, false);
    let (h, held, _host) = window_on(&ws);
    let ui = &h.ui;
    // a run that writes the exports: the roster, added from Setup
    click(ui, &tab(ui, "Setup"));
    press(ui, "4 Roster, warnings");
    type_into(ui, "Roster file", roster.to_str().unwrap());
    press(ui, "Add roster and match students");
    run_held(&h, &held);
    let (_, text, steps) = notice(ui);
    assert!(
        steps.contains(&"Open exports folder".to_string()),
        "{steps:?}"
    );
    assert!(!text.contains("written before"), "{text}");
    // a decision after them, the notice on show
    click(ui, &tab_of(ui, "Review"));
    press(ui, "Accept suggestion");
    let (_, text, _) = notice(ui);
    assert!(
        text.ends_with(
            "these exports were written before your latest changes: Write all exports in \
             Results updates them"
        ),
        "{text}"
    );
    click(ui, &tab(ui, "Results"));
    press(ui, "Export…");
    let note = "These exports were written before your latest changes: Write all exports in \
                Results updates them";
    one(ui, note);
    // the bar's button says it too (the notice has its own)
    let said: Vec<String> = by_label(ui, "Open exports folder")
        .iter()
        .filter_map(|e| e.accessible_description())
        .map(|d| d.to_string())
        .filter(|d| !d.is_empty())
        .collect();
    assert_eq!(said, [note]);
    press(ui, "Write all exports");
    assert!(by_label(ui, note).is_empty(), "written again: current");
    let (_, text, _) = notice(ui);
    assert!(!text.contains("written before"), "{text}");
}

// ------------------------------------------------------------------ cancelling a run (D7)

/// D7: a new exam being graded is cancelled from the Scan screen after a confirmation: nothing
/// is saved (no exam folder is left, so the next run takes the same name), the scans are
/// untouched, the notice says so and offers the card again, filled as it was.
#[test]
fn cancel_grading_from_the_scan_screen_leaves_no_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let scans_before = files_in(&w.scans);
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Scan);
    assert!(by_label(ui, "Cancel grading").is_empty(), "asked first");
    // the confirmation, and keeping it running (in the smallest window)
    smallest_window(ui);
    fits(ui, "Cancel grading…");
    press(ui, "Cancel grading…");
    for l in ["Keep grading", "Cancel grading"] {
        fits(ui, l);
    }
    window_size(ui, 1440, 900);
    press(ui, "Keep grading");
    assert!(by_label(ui, "Cancel grading").is_empty());
    assert!(w.h.app.borrow().grading_in_progress());
    // cancelled
    press(ui, "Cancel grading…");
    press(ui, "Cancel grading");
    assert!(message(ui).starts_with("Cancelling"), "{}", message(ui));
    assert!(by_label(ui, "Cancel grading").is_empty(), "the bar closed");
    run_held(&w.h, &w.held);
    let (title, text, steps) = notice(ui);
    assert_eq!(
        format!("{title}. {text}"),
        "Grading cancelled. Nothing was saved; your scan files are unchanged."
    );
    assert_eq!(steps, ["Edit and grade again"]);
    assert!(!w.ws.join("cs-213-quiz-1").exists(), "no folder is left");
    assert_eq!(files_in(&w.ws), Vec::<String>::new());
    assert_eq!(files_in(&w.scans), scans_before, "the scans are untouched");
    assert!(w.h.app.borrow().dashboard().exams.is_empty(), "not listed");
    assert!(w.h.app.borrow().can_create_exam());
    assert_eq!(w.h.app.borrow().create_exam_blocker(), None);
    assert!(by_label(ui, "Cancel grading…").is_empty());

    // Edit and grade again: the card as it was, its answers included, checked again, graded
    // into the same folder
    press(ui, "Edit and grade again");
    assert!(!ui.global::<App>().get_notice_open());
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Exams);
    let ex = ui.global::<ExamsVM>();
    assert!(ex.get_new_open());
    assert_eq!(ex.get_new_scans(), w.scans.to_str().unwrap());
    assert_eq!(ex.get_new_title(), "Quiz 1");
    settle_card(&w.h, &w.held);
    assert!(by_label(ui, PROVISIONAL).is_empty(), "answered as before");
    let key = card_row(ui, "Answer key");
    assert!(key.contains("— your answer"), "{key}");
    assert!(enabled(ui, "Grade exam"), "{}", ex.get_new_block());
    assert!(card_row(ui, "Folder").contains("cs-213-quiz-1 —"));
    press(ui, "Grade exam");
    run_held(&w.h, &w.held);
    assert!(w.ws.join("cs-213-quiz-1/project.omrproj").is_dir());
}

/// D7: closing the window while an exam is being graded asks first; kept, the run goes on;
/// cancelled, the window waits for it to stop, then closes, and nothing half-saved is left.
#[test]
fn closing_the_window_while_grading_asks_and_cancels() {
    use slint::platform::WindowEvent;
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    // nothing runs: the window closes at once
    assert!(w.h.shell.close_requested());
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible(), "the window stays");
    assert!(app.get_close_open());
    smallest_window(ui);
    for l in ["Close Mizan?", "Keep it running", "Cancel it and close"] {
        fits(ui, l);
    }
    let card = one(ui, "Close Mizan?");
    let why = card
        .accessible_description()
        .unwrap_or_default()
        .to_string();
    assert!(
        why.starts_with("CS 213 Quiz 1 is being graded. Closing cancels it"),
        "{why}"
    );
    press(ui, "Keep it running");
    assert!(!app.get_close_open());
    assert!(w.h.app.borrow().grading_in_progress());

    ui.window().dispatch_event(WindowEvent::CloseRequested);
    press(ui, "Cancel it and close");
    assert!(app.get_closing(), "waiting for the run to stop");
    assert!(ui.window().is_visible());
    // asked again while it waits: the window stays until the run has stopped (closed at
    // once, the run would be killed with the process and leave a half-made exam behind)
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible(), "still waiting for the run");
    assert!(app.get_closing() && app.get_close_open());
    assert!(w.h.app.borrow().grading_in_progress());
    run_held(&w.h, &w.held);
    assert!(!ui.window().is_visible(), "closed once the run stopped");
    assert!(!w.ws.join("cs-213-quiz-1").exists(), "nothing half-saved");
}

/// Runs the held work to its end on a thread without letting the window fold it in: the state
/// of a run that has read its pages and is being saved, as a click finds it then.
fn run_held_unseen(held: &Held) {
    let work = held.borrow_mut().pop().expect("work was started");
    std::thread::spawn(work).join().expect("worker");
}

/// D7: once the pages are read the run is being saved: Cancel grading is too late, says so
/// and never promises that nothing is saved; the exam is graded and kept, as the notice says.
#[test]
fn a_cancel_once_the_run_is_being_saved_is_too_late() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    let exam = w.h.app.borrow().default_exam().expect("the run's exam");
    run_held_unseen(&w.held);
    assert!(w.h.app.borrow().saving(), "being saved, not folded in yet");
    assert!(!w.h.app.borrow().can_cancel(&exam), "no Cancel once saving");
    // the Scan screen as last shown still offers it: the click is answered honestly
    press(ui, "Cancel grading…");
    press(ui, "Cancel grading");
    let msg = message(ui);
    assert!(
        msg.starts_with("Too late to cancel: the pages are read and CS 213 Quiz 1 is being saved"),
        "{msg}"
    );
    assert!(!msg.contains("nothing is saved"), "{msg}");
    assert!(
        by_label(ui, "Cancel grading…").is_empty(),
        "the button went"
    );
    let log: Vec<String> = ui
        .global::<ScanVM>()
        .get_activity()
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert!(!log.iter().any(|l| l.contains("cancelling")), "{log:?}");
    w.h.shell.pump();
    let (title, text, _) = notice(ui);
    assert_eq!(title, "CS 213 Quiz 1 is graded");
    assert!(!text.contains("Nothing was saved"), "{text}");
    assert!(w.ws.join("cs-213-quiz-1/project.omrproj").is_dir());
}

/// D7: closing the window while the run is being saved: a cancel is too late, so the window
/// says the exam is kept and offers to close once it is saved, never that nothing is kept.
#[test]
fn closing_while_the_run_is_being_saved_waits_and_keeps_it() {
    use slint::platform::WindowEvent;
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    run_held_unseen(&w.held);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible());
    let why = one(ui, "Close Mizan?")
        .accessible_description()
        .unwrap_or_default()
        .to_string();
    assert!(
        why.starts_with("CS 213 Quiz 1 is being graded. Its pages are read")
            && why.contains("so it is kept"),
        "{why}"
    );
    assert!(!why.contains("keeps nothing"), "{why}");
    assert!(by_label(ui, "Cancel it and close").is_empty());
    press(ui, "Close once saved");
    assert!(app.get_closing() && ui.window().is_visible());
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible(), "still waiting");
    w.h.shell.pump();
    assert!(!ui.window().is_visible(), "closed once saved");
    assert!(w.ws.join("cs-213-quiz-1/project.omrproj").is_dir(), "kept");
}

/// D7: should the window's event loop end while a run is still going, the run is cancelled
/// and waited for before the program ends: a new exam leaves nothing half-made behind.
#[test]
fn a_run_still_going_at_exit_is_cancelled_and_waited_for() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    let work = w.held.borrow_mut().pop().expect("the run");
    let worker = std::thread::spawn(work);
    let backend: &std::cell::RefCell<dyn Backend> = &*w.h.app;
    assert!(shell_slint::backend::wind_down(
        backend,
        std::time::Duration::from_secs(300)
    ));
    worker.join().expect("worker");
    assert!(!w.h.app.borrow().grading_in_progress());
    assert!(!w.ws.join("cs-213-quiz-1").exists(), "nothing half-made");
}

// ------------------------------------------------------------------ roster after grading (D8)

/// The roster CSV of the synthetic exam `spec` (`sheets + 2` students, two absent), with a
/// student listed twice when `duplicate`.
fn roster_file(spec: &synth::Spec, path: &Path, duplicate: bool) {
    let mut csv = "student_id,name,section\n".to_string();
    let rows = synth::roster(spec);
    for r in &rows {
        csv.push_str(&format!("{},{},{}\n", r.id, r.name, r.section));
    }
    if duplicate {
        csv.push_str(&format!("{},Someone Else,171\n", rows[0].id));
    }
    std::fs::write(path, csv).expect("roster file");
}

/// D8: Setup's step 4 Roster opens the roster bar; the roster chosen in the dialog is checked,
/// then every booklet is matched again in the background, as a run on the Scan screen; when it
/// ends the exam has its roster, the step is done and the notice says what was matched.
#[test]
fn setup_roster_step_adds_a_roster_in_the_background() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let roster = dir.path().join("roster.csv");
    roster_file(&QUIZ_1, &roster, false);
    let (h, held, host) = window_on(&ws);
    let ui = &h.ui;
    let su = ui.global::<SetupVM>();
    click(ui, &tab(ui, "Setup"));
    assert!(!su.get_has_roster());
    let step = one(ui, "4 Roster, warnings");
    assert!(
        step.accessible_description()
            .unwrap_or_default()
            .starts_with("No roster"),
        "{:?}",
        step.accessible_description()
    );
    click(ui, &step);
    assert!(su.get_roster_open());
    smallest_window(ui);
    for l in [
        "4 Roster, warnings",
        "Roster file",
        "Browse for Roster file",
        "Add roster and match students",
        "Replace key…",
    ] {
        fits(ui, l);
    }
    host.will_pick(&[&roster]);
    press(ui, "Browse for Roster file");
    let req = host.last_request();
    assert_eq!(
        (req.target, req.kind),
        (PickTarget::SetupRoster, PickKind::File)
    );
    assert_eq!(
        req.filters[0],
        ("Roster".to_string(), texts(&["csv", "xlsx"]))
    );
    assert_eq!(su.get_roster_path(), roster.to_str().unwrap());
    press(ui, "Add roster and match students");
    assert!(
        message(ui).starts_with("Adding the roster (8 students)"),
        "{}",
        message(ui)
    );
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Scan);
    assert!(h.app.borrow().grading_in_progress());
    assert!(
        ui.global::<ScanVM>()
            .get_batch_label()
            .ends_with("matching students to the roster"),
        "{}",
        ui.global::<ScanVM>().get_batch_label()
    );
    assert!(
        by_label(ui, "Cancel grading…").is_empty(),
        "a graded exam's run"
    );
    run_held(&h, &held);

    let exam = h.app.borrow().default_exam().expect("the exam");
    let (students, matched) = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            (
                p.session.definition.roster.as_ref().map_or(0, Vec::len),
                p.session
                    .booklets
                    .iter()
                    .filter(|b| b.student().is_some())
                    .count(),
            )
        })
        .expect("reopened");
    assert_eq!(students, 8);
    assert!(matched >= 4, "{matched} booklets matched");
    let (title, text, steps) = notice(ui);
    assert_eq!(title, "Roster added to CS 213 Quiz 1");
    assert!(
        text.starts_with(&format!(
            "8 students on the roster · {matched} booklets matched · "
        )),
        "{text}"
    );
    assert!(text.contains("exports in "), "{text}");
    assert!(steps.contains(&"Open results".to_string()), "{steps:?}");
    // the step is done
    click(ui, &tab(ui, "Setup"));
    assert!(su.get_has_roster());
    let step = one(ui, "4 Roster, done");
    assert!(
        step.accessible_description()
            .unwrap_or_default()
            .starts_with("Roster: 8 students"),
        "{:?}",
        step.accessible_description()
    );
}

/// D8: while a roster is matched, a change elsewhere is refused naming that run as the Scan
/// screen names it: the exam's name (never its ID), and no scans said to be added.
#[test]
fn a_change_during_the_roster_run_names_that_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let roster = dir.path().join("roster.csv");
    roster_file(&QUIZ_1, &roster, false);
    let (h, held, _host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    press(ui, "4 Roster, warnings");
    type_into(ui, "Roster file", roster.to_str().unwrap());
    press(ui, "Add roster and match students");
    assert!(h.app.borrow().grading_in_progress());
    click(ui, &tab_of(ui, "Review"));
    chord(ui, Key::Control, "z");
    let msg = message(ui);
    assert!(
        msg.starts_with(
            "The roster of CS 213 Quiz 1 is being matched: nothing can change until the Scan \
             screen says it is complete"
        ),
        "{msg}"
    );
    assert!(!msg.contains(&h.exam.0), "{msg}");
    run_held(&h, &held);
}

/// D7: scans added from a mistyped folder fail: the notice and the Scan log say what went
/// wrong and what to do in this window, with no command line, exit code, error kind or
/// package path.
#[test]
fn a_failed_run_says_what_to_do_in_this_window() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let (h, held, _host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Scan"));
    press(ui, "Add scans…");
    let typo = dir.path().join("day 2 scnas");
    type_into(ui, "Scan files or folders to add", typo.to_str().unwrap());
    press(ui, "Add and regrade");
    run_held(&h, &held);
    let (title, text, steps) = notice(ui);
    assert_eq!(title, "Adding scans to CS 213 Quiz 1 failed");
    assert_eq!(
        format!("{:?}", ui.global::<App>().get_notice_tone()),
        "Error",
        "announced at once"
    );
    assert!(
        text.contains("day 2 scnas: no such file or folder")
            && text.contains("Nothing was added; the exam is as it was")
            && text.contains("Add scans…"),
        "{text}"
    );
    assert_eq!(steps, ["Show the Scan log"]);
    let log = ui
        .global::<ScanVM>()
        .get_activity()
        .row_data(0)
        .expect("the Scan log")
        .to_string();
    assert!(log.contains("no such file or folder"), "{log}");
    for t in [&text, &log] {
        for bad in ["exit code", "omr ", "--project", ".omrproj", "input:"] {
            assert!(!t.contains(bad), "{bad:?} in {t}");
        }
    }
}

/// D8: exports a run cannot write (results.csv open in Excel on Windows) are named in the
/// notice with what to do, in plain words; the roster itself is added.
#[test]
fn exports_that_cannot_be_written_are_named_with_what_to_do() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let roster = dir.path().join("roster.csv");
    roster_file(&QUIZ_1, &roster, false);
    // a folder stands in for a results file another program keeps open
    let exam_dir = ws.join(QUIZ_1.folder);
    for name in ["results.csv", "results-INFERRED.csv"] {
        let _ = std::fs::remove_file(exam_dir.join(name));
        std::fs::create_dir(exam_dir.join(name)).expect("stand-in");
    }
    let (h, held, _host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    press(ui, "4 Roster, warnings");
    type_into(ui, "Roster file", roster.to_str().unwrap());
    press(ui, "Add roster and match students");
    run_held(&h, &held);
    let (title, text, _) = notice(ui);
    assert_eq!(title, "Roster added to CS 213 Quiz 1");
    assert!(
        text.contains("The exports were not written: results")
            && text.contains(
                ".csv could not be written (Is a directory (os error 21)); if it is open in \
                 another program, such as Excel, close it. Write them again with Write all \
                 exports in Results"
            ),
        "{text}"
    );
    assert!(
        !text.contains("i/o:") && !text.contains("writes them"),
        "{text}"
    );
}

/// D8: a roster the core refuses (a student number listed twice) is refused at once, in the
/// status line, with nothing started and the exam as it was; the bar stays open to fix it.
#[test]
fn a_refused_roster_changes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ws = exams_folder(&dir.path().join("exams"), &[&QUIZ_1]);
    let roster = dir.path().join("roster.csv");
    roster_file(&QUIZ_1, &roster, true);
    let (h, held, _host) = window_on(&ws);
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    press(ui, "4 Roster, warnings");
    type_into(ui, "Roster file", roster.to_str().unwrap());
    press(ui, "Add roster and match students");
    let msg = message(ui);
    assert!(
        msg.starts_with("roster refused") && msg.contains("appears 2 times"),
        "{msg}"
    );
    assert!(held.borrow().is_empty(), "nothing started");
    assert!(!h.app.borrow().grading_in_progress());
    assert_eq!(ui.global::<App>().get_page(), shell_slint::ui::Page::Setup);
    assert!(
        ui.global::<SetupVM>().get_roster_open(),
        "the bar stays to fix it"
    );
    let exam = h.app.borrow().default_exam().expect("the exam");
    let roster_now = h
        .app
        .borrow()
        .with_project(&exam, |p| p.session.definition.roster.is_some())
        .expect("still open");
    assert!(!roster_now);
}

// ------------------------------------------------------------- the workflow's screenshots

/// A spawner that starts the work on a thread at once, as the binary's does (a screenshot waits
/// for it with `settle`).
fn thread_spawner() -> shell_slint::app_backend::Spawn {
    Box::new(|work| {
        std::thread::Builder::new()
            .spawn(work)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// The New-exam card's screenshots are made from an exam's files, typed as an instructor types
/// them (`~` for the home folder), its scans checked before the capture (never "Checking…"),
/// with the Browse buttons the app shows: with the scans alone the answer key is asked and
/// Grade exam waits; with the key, roster and CLO files beside them nothing is asked, the title
/// and course come from the scans' folder name and the card's folder reads `~` too.
#[test]
fn screenshots_make_the_card_from_an_exams_files() {
    let Some(home) = std::env::home_dir() else {
        return;
    };
    let dir = tempfile::Builder::new()
        .prefix(".mizan-test-")
        .tempdir_in(&home)
        .expect("temp dir in the home folder");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::NEXT
    };
    let files = synth::write_exam_files(&spec, &dir.path().join("to-grade"));
    let ws = dir.path().join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let mut app = AppBackend::workspace(&ws, "principal:a11");
    app.set_spawner(thread_spawner());
    let h = harness_in(app);
    let ui = &h.ui;
    let ex = ui.global::<ExamsVM>();
    let given = screenshot::ShotFiles {
        card: Some(screenshot::CardFiles {
            scans: vec![files.scans.clone()],
            key: files.key.clone(),
            roster: files.roster.clone(),
            clo: files.clo.clone(),
            id_scheme: "digits:10".into(),
        }),
        ..Default::default()
    };
    let shots = screenshot::card_shots();
    let shot = |name: &str| *shots.iter().find(|s| s.name == name).expect(name);
    h.shell.set_host(screenshot::display_host());
    let stage = |shot: &screenshot::Shot| {
        screenshot::stage_with(&h.shell, &*h.app, shot, &given);
        if h.app.borrow_mut().settle() {
            h.shell.refresh_all();
        }
    };
    let sep = std::path::MAIN_SEPARATOR;
    let rel = dir
        .path()
        .strip_prefix(&home)
        .expect("below home")
        .display();

    stage(&shot("exams-new-ask"));
    assert!(ex.get_new_open());
    assert_eq!(ex.get_survey_state(), 2, "{}", ex.get_survey_text());
    assert_eq!(
        ex.get_new_scans(),
        format!("~{sep}{rel}{sep}to-grade{sep}CS213 Quiz 1")
    );
    assert_eq!(ex.get_new_key(), "", "the scans alone");
    let asks: Vec<String> = ex
        .get_card_asks()
        .iter()
        .map(|a| a.label.to_string())
        .collect();
    assert_eq!(asks, ["Answer key"]);
    assert!(!enabled(ui, "Grade exam"));
    assert!(!browse_buttons(ui).is_empty(), "the app's Browse buttons");

    stage(&shot("exams-new-ready"));
    assert_eq!(ex.get_survey_state(), 2, "{}", ex.get_survey_text());
    assert_eq!(
        ex.get_new_roster(),
        format!("~{sep}{rel}{sep}to-grade{sep}roster.csv")
    );
    assert_eq!(ex.get_card_asks().row_count(), 0, "nothing asked");
    assert!(enabled(ui, "Grade exam"));
    // the scheme typed under Advanced options, open: the synthetic numbers fit it
    assert!(ex.get_new_advanced());
    assert!(
        card_row(ui, "Student ID").starts_with("Student ID: 10-digit number — typed"),
        "{}",
        card_row(ui, "Student ID")
    );
    for (label, value) in [
        ("Answer key", "Answer key: key.csv — key file".to_string()),
        ("Title", "Title: Quiz 1 — folder name".into()),
        ("Course", "Course: CS 213 — folder name".into()),
        (
            "Folder",
            format!("Folder: ~{sep}{rel}{sep}exams{sep}cs-213-quiz-1 — "),
        ),
    ] {
        let row = card_row(ui, label);
        assert!(row.starts_with(&value), "{row}");
    }

    // the card as it opens: nothing typed
    stage(&screenshot::Shot {
        card: 0,
        ..shot("exams-new-ready")
    });
    assert_eq!(ex.get_new_scans(), "");
    assert_eq!(ex.get_new_clo(), "");
    assert!(!ex.get_new_advanced(), "folded again");
    assert_eq!(ex.get_survey_state(), 0);
}

/// The notice's screenshot (a `--grade` run) shows the notice of the run that ended on its own
/// shot only: the other shots hide it, and the next theme's shot shows it again.
#[test]
fn screenshots_show_the_notice_on_its_shot_only() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let app = ui.global::<App>();
    press(ui, "New exam");
    grade_with_card(&w.h, &w.held, &quiz_1(&w.scans));
    // the run ends without the window taking its notice, as a screenshot run waits for it
    let work = w.held.borrow_mut().pop().expect("the run");
    std::thread::spawn(work).join().expect("worker");
    let t0 = std::time::Instant::now();
    while w.h.app.borrow().grading_in_progress() {
        assert!(t0.elapsed() < std::time::Duration::from_secs(60));
        w.h.app.borrow_mut().pump();
    }
    assert!(!app.get_notice_open());
    let scan = screenshot::real_shots(&*w.h.app.borrow())
        .into_iter()
        .find(|s| s.name == "scan")
        .expect("the scan shot");
    let notice = screenshot::notice_shot();
    // light, then dark
    for _ in 0..2 {
        screenshot::stage(&w.h.shell, &*w.h.app, &scan);
        assert!(!app.get_notice_open(), "not on another shot");
        screenshot::stage(&w.h.shell, &*w.h.app, &notice);
        assert!(app.get_notice_open());
        assert_eq!(app.get_notice_title(), "CS 213 Quiz 1 is graded");
        assert_eq!(app.get_page(), shell_slint::ui::Page::Scan);
    }
}

/// An exams folder with no exam has one screenshot, its empty state, with the actions the app
/// shows (Open another folder… needs the folder dialog the screenshots show).
#[test]
fn screenshots_of_an_empty_folder_show_its_empty_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let w = workspace(dir.path());
    let ui = &w.h.ui;
    let shots = screenshot::real_shots(&*w.h.app.borrow());
    let names: Vec<&str> = shots.iter().map(|s| s.name).collect();
    assert_eq!(names, ["exams-empty"]);
    w.h.shell.set_host(screenshot::display_host());
    screenshot::stage(&w.h.shell, &*w.h.app, &shots[0]);
    one(ui, "No exams in this folder yet");
    for action in [
        "New exam…",
        "Open another folder…",
        "Import from Remark export…",
    ] {
        one(ui, action);
    }
}
