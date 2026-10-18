//! A11 for the Remark migration (spec §3.5, PAR-09) and the parallel run (§23.7), driven
//! headless through accessible labels over the real core: synthetic Remark graded workbooks
//! (`tests/support/synthetic_remark.rs`: invented rows in the documented export layout, built
//! from the synthetic exam's own sheets; never a real export) read on the New-exam form and in
//! Setup, shown for review (the key as counts until revealed, exact points per question, the
//! learning objectives "imported, needs review", the key-sheet rows excluded, refusals with
//! their remedy) and applied; and the parallel run in Results with its crops, under the
//! Restricted / Q-08 banner, its report written where asked.
//!
//! Each test runs on its own thread with its own Slint testing platform.

mod support;

use i_slint_backend_testing::{AccessibleRole, ElementQuery};
use shell_slint::app_backend::{AppBackend, Spawn};
use shell_slint::backend::{Backend, ExamId};
use shell_slint::bind::Shell;
use shell_slint::ui::{App, AppWindow, ExamsVM, Page, RemarkVM, Theme};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::a11::{by_label, one, tab, with_prefix};
use support::synthetic_exam::{self as synth, MIDTERM};
use support::synthetic_remark as remark;

struct Harness {
    ui: AppWindow,
    app: Rc<RefCell<AppBackend>>,
    shell: Rc<Shell>,
}

fn harness_for(app: AppBackend) -> Harness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let app = Rc::new(RefCell::new(app));
    let dynamic: Rc<RefCell<dyn Backend>> = app.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    Harness { ui, app, shell }
}

type Held = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;

/// A spawner that holds the work for the test to run (the binary's threads, R4).
fn held_spawner() -> (Spawn, Held) {
    let held: Held = Rc::default();
    let h = held.clone();
    (
        Box::new(move |work| {
            h.borrow_mut().push(work);
            Ok(())
        }),
        held,
    )
}

fn run_held(h: &Harness, held: &Held) {
    let work = held.borrow_mut().pop().expect("work was started");
    let worker = std::thread::spawn(work);
    let t0 = Instant::now();
    while h.app.borrow().grading_in_progress() {
        assert!(
            t0.elapsed() < Duration::from_secs(300),
            "the batch never finished"
        );
        h.shell.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.join().expect("worker");
    h.shell.pump();
}

fn message(ui: &AppWindow) -> String {
    ui.global::<App>().get_message().to_string()
}

fn joined(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_str().expect("utf-8 path"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Every interactive control on screen carries a label, and no two the same (DESIGN §8, §10),
/// and none ends past the window's right edge.
fn labels_unique_and_inside(ui: &AppWindow, what: &str) {
    use AccessibleRole as R;
    let interactive = [
        R::Button,
        R::Checkbox,
        R::Combobox,
        R::Slider,
        R::Spinbox,
        R::Tab,
        R::TextInput,
        R::Switch,
        R::RadioButton,
    ];
    let width = ui.window().size().width as f32;
    let height = ui.window().size().height as f32;
    // the status bar stays inside the window (DESIGN §9: nothing pushes it out)
    let bars: Vec<_> =
        i_slint_backend_testing::ElementHandle::find_by_element_type_name(ui, "StatusBar")
            .collect();
    assert!(!bars.is_empty(), "{what}: no status bar");
    for b in bars {
        let (p, sz) = (b.absolute_position(), b.size());
        assert!(
            p.y >= 0.0 && p.y + sz.height <= height + 0.5,
            "{what}: status bar at y {} + {} is outside the window",
            p.y,
            sz.height
        );
    }
    let mut seen: BTreeMap<(String, i64, i64), ()> = BTreeMap::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        let Some(role) = e.accessible_role() else {
            continue;
        };
        if !interactive.contains(&role) || e.computed_opacity() <= 0.0 || e.size().width <= 0.0 {
            continue;
        }
        let (p, sz) = (e.absolute_position(), e.size());
        let label = e
            .accessible_label()
            .map(|l| l.to_string())
            .unwrap_or_default();
        assert!(
            !label.trim().is_empty(),
            "{what}: a control without a label"
        );
        assert!(
            p.x + sz.width <= width + 0.5,
            "{what}: {label:?} ends at x {} past the window",
            p.x + sz.width
        );
        seen.insert((label, (p.x * 4.0) as i64, (p.y * 4.0) as i64), ());
    }
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (l, _, _) in seen.keys() {
        *count.entry(l.as_str()).or_default() += 1;
    }
    let dup: Vec<_> = count.iter().filter(|(_, n)| **n > 1).collect();
    assert!(dup.is_empty(), "{what}: duplicate labels {dup:?}");
}

/// Reads the typed workbooks; a small synthetic class may not tell a learning objective's
/// questions apart (F4: the equal split is never assumed), and then the refusal names the
/// objective and suggests its split, which the instructor gives as an explicit choice.
fn read_export_with_suggested_points(ui: &AppWindow) {
    one(ui, "Read export").invoke_accessible_default_action();
    let rm = ui.global::<RemarkVM>();
    if !rm.get_refused() {
        // a class large enough to tell every question apart
        assert!(rm.get_has(), "{}", message(ui));
        return;
    }
    assert!(
        rm.get_refusal().contains("learning objective")
            && rm.get_refusal().contains("split equally"),
        "{}",
        rm.get_refusal()
    );
    assert!(!rm.get_suggestion().is_empty(), "the split is suggested");
    one(ui, "Use the suggested points").invoke_accessible_default_action();
    assert!(rm.get_has() && !rm.get_refused(), "{}", message(ui));
}

/// The rows of the import's table (question list items), by label.
fn import_rows(ui: &AppWindow) -> Vec<String> {
    with_prefix(ui, "Q")
        .into_iter()
        .map(|(l, _)| l)
        .filter(|l| l.contains(" points"))
        .collect()
}

/// §3.5 on the New-exam form: Remark's graded workbooks are read through omr-app and shown for
/// review (the key as counts until revealed, exact points per question and version, the
/// learning objectives "imported, needs review", the key sheet Remark graded as a student
/// excluded); *Use this import* fills the form; a key file beside it is refused with what to
/// do; the exam is graded with the imported key, its points per version and the objectives as
/// proposed CLO mappings.
#[test]
fn req_a11_remark_import_on_the_new_exam_form_grades_with_its_key_points_and_objectives() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 12,
        ..MIDTERM
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let books = remark::write_books(&spec, &dir.path().join("remark"));
    let ws = dir.path().join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11-instructor");
    app.set_spawner(spawn);
    let h = harness_for(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 1600));

    one(ui, "New exam").invoke_accessible_default_action();
    one(ui, "Import from Remark export…").invoke_accessible_default_action();
    assert!(
        ui.global::<RemarkVM>().get_new_panel(),
        "the import panel opens"
    );
    one(ui, "Remark workbooks").set_accessible_value(joined(&books));
    read_export_with_suggested_points(ui);
    let rm = ui.global::<RemarkVM>();
    assert!(message(ui).contains("nothing changes until you use the import"));

    // the key is counts only until revealed; every row carries its exact points
    // (the list is virtualised: the rows on screen are checked through their labels, all of
    // them through the view-model)
    assert_eq!(rm.get_rows().row_count(), usize::from(synth::QUESTIONS));
    let rows = import_rows(ui);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|l| !l.contains(", key ")), "{rows:?}");
    let first = rows.iter().find(|l| l.starts_with("Q01:")).expect("Q01");
    assert!(
        first.contains("1 points") && first.contains("learning objective LO1"),
        "{first}"
    );
    assert!(rm.get_key_counts().contains("30 entries"));
    one(ui, "Reveal key").invoke_accessible_default_action();
    let key_a = |q: u16| char::from(b'A' + synth::key(&spec, 0, q)).to_string();
    let first = import_rows(ui)
        .into_iter()
        .find(|l| l.starts_with("Q01:"))
        .expect("Q01");
    assert!(first.contains(&format!(", key {}", key_a(0))), "{first}");
    one(ui, "Hide key").invoke_accessible_default_action();
    // version B carries other points per question (its objectives are in another order)
    one(ui, "Remark import version B, 30 questions, 25 points").invoke_accessible_default_action();
    let first_b = import_rows(ui)
        .into_iter()
        .find(|l| l.starts_with("Q01:"))
        .expect("Q01");
    assert!(
        first_b.contains("0.5 points") && first_b.contains("LO2"),
        "{first_b}"
    );
    // the learning objectives, imported for review; the key sheet excluded
    let objectives: Vec<String> = with_prefix(ui, "Learning objective ")
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    // (a scrolled list: the objectives on screen through their labels, all of them through the
    // view-model)
    assert_eq!(rm.get_objectives().row_count(), 4);
    assert!(
        objectives
            .iter()
            .any(|l| l.starts_with("Learning objective LO1 (1-8): version A")),
        "{objectives:?}"
    );
    assert!(
        objectives
            .iter()
            .filter(|l| l.contains("): "))
            .all(|l| l.ends_with("imported, needs review"))
    );
    let excluded: Vec<String> = with_prefix(ui, "Key sheet excluded: ")
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    // the instructor's key sheet (one repeated digit), and any student whose answers are the
    // key with full marks (reported for a check, never a student); IDs masked
    assert_eq!(
        excluded
            .iter()
            .filter(|l| l.contains("one repeated digit"))
            .count(),
        1,
        "{excluded:?}"
    );
    assert!(
        excluded
            .iter()
            .all(|l| l.contains("ID •") && !l.contains("0000000000")),
        "masked: {excluded:?}"
    );
    labels_unique_and_inside(ui, "New exam · Remark import");

    // use it: the form carries the questions and versions, and says where the key comes from
    one(ui, "Use this import").invoke_accessible_default_action();
    let ex = ui.global::<ExamsVM>();
    assert!(!rm.get_new_panel());
    assert_eq!(ex.get_new_questions(), "30");
    assert_eq!(ex.get_new_forms(), "A,B");
    assert!(rm.get_new_using());
    assert_eq!(with_prefix(ui, "From the Remark import").len(), 1);
    one(ui, "Scan files or folders").set_accessible_value(scans.to_str().unwrap());
    one(ui, "Exam title").set_accessible_value(spec.title);
    one(ui, "Course code").set_accessible_value(spec.course_code);
    // a second key source is refused with what to do; nothing starts
    one(ui, "Answer key file").set_accessible_value("/tmp/key.csv");
    one(ui, "Grade exam").invoke_accessible_default_action();
    assert!(
        message(ui).contains("the key comes from the Remark import"),
        "{}",
        message(ui)
    );
    assert!(held.borrow().is_empty());
    one(ui, "Answer key file").set_accessible_value("");
    one(ui, "Grade exam").invoke_accessible_default_action();
    assert!(
        message(ui).contains("come from the Remark import"),
        "{}",
        message(ui)
    );
    run_held(&h, &held);
    assert_eq!(h.app.borrow().grading_error(), None);
    let exam = h.app.borrow().default_exam().expect("the new exam");
    let (keys, points, mapping) = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            let d = &p.session.definition;
            let keys = match &d.key {
                omr_app::exam::KeySource::Provided { forms } => forms.clone(),
                k => panic!("key {k:?}"),
            };
            let mapping: Vec<(u16, String, String, String)> = d
                .outcomes
                .as_ref()
                .expect("objectives")
                .mapping
                .iter()
                .map(|m| {
                    (
                        m.question,
                        m.clo.clone(),
                        m.form.clone(),
                        m.imported.clone(),
                    )
                })
                .collect();
            (keys, d.points_by_form(), mapping)
        })
        .expect("project");
    for (fi, k) in keys.iter().enumerate() {
        for q in 0..synth::QUESTIONS {
            assert_eq!(
                k.answers[usize::from(q)],
                char::from(b'A' + synth::key(&spec, fi, q)).to_string()
            );
        }
    }
    for (fi, p) in points.iter().enumerate() {
        for (g, a, z, v) in remark::groups(fi) {
            for q in a..=z {
                let got: f64 = p[usize::from(q - 1)].parse().expect("number");
                assert_eq!(got, v, "version {fi} Q{q} ({g})");
            }
        }
    }
    assert!(
        mapping.iter().all(|m| m.3 == "Remark import"),
        "{mapping:?}"
    );
    assert!(
        mapping
            .iter()
            .any(|m| m.0 == 1 && m.1 == "LO1" && m.2 == "A")
    );
    assert!(
        mapping
            .iter()
            .any(|m| m.0 == 1 && m.1 == "LO2" && m.2 == "B")
    );
    // the import is used up: the next form starts without it
    assert!(!ui.global::<RemarkVM>().get_new_using());

    // F5: the imported rows are approved in Setup by an audited act; the TP-154 tables leave
    // the draft watermark without grading again
    let draft = |h: &Harness| {
        h.app
            .borrow()
            .with_project(&exam, |p| {
                p.outcomes().expect("outcomes").b1.draft_watermark
            })
            .expect("project")
    };
    assert!(draft(&h), "imported rows are proposed");
    h.shell.select_exam(&exam);
    tab(ui, "Setup").invoke_accessible_default_action();
    assert!(
        ui.global::<shell_slint::ui::SetupVM>()
            .get_mappings_proposed()
            > 0
    );
    one(ui, "Approve imported mappings").invoke_accessible_default_action();
    assert!(message(ui).contains("Approved"), "{}", message(ui));
    assert!(!draft(&h), "approved: no draft watermark");
    assert_eq!(
        ui.global::<shell_slint::ui::SetupVM>()
            .get_mappings_proposed(),
        0
    );
    assert!(by_label(ui, "Approve imported mappings").is_empty());
    let approvals = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            p.store()
                .expect("store")
                .events()
                .expect("events")
                .iter()
                .filter(|e| e.event_type == "MappingApproved")
                .count()
        })
        .expect("project");
    assert_eq!(approvals, 1, "an audited approval");

    // F8: Setup's *Replace key…* reads a key file with a points column per version (the exam
    // carries other points per version): the same keys and points are taken, a new key version
    let key_file = dir.path().join("key-per-version.csv");
    let mut text = String::from("question,A,B,points_A,points_B\n");
    for (q, (pa, pb)) in points[0].iter().zip(&points[1]).enumerate() {
        text.push_str(&format!(
            "{},{},{},{pa},{pb}\n",
            q + 1,
            keys[0].answers[q],
            keys[1].answers[q],
        ));
    }
    std::fs::write(&key_file, text).expect("key file");
    let msg = h
        .app
        .borrow_mut()
        .replace_key(&exam, &key_file)
        .expect("a per-version points file is read");
    assert!(msg.contains("Key version 2"), "{msg}");
}

/// A refusal shows what to do (R10): Remark's own files are never opened; totals that leave
/// two questions' points open name them and suggest the points a determined version leaves,
/// which only the instructor's explicit choice gives; then every total is still checked.
#[test]
fn req_a11_remark_import_refusals_name_the_remedy_and_the_suggested_points() {
    let dir = tempfile::tempdir().expect("temp dir");
    let books = remark::underdetermined(&dir.path().join("remark"));
    let rmx = dir.path().join("All_Section_A.rmx");
    std::fs::write(&rmx, b"not opened").expect("write");
    let ws = dir.path().join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11-instructor");
    app.set_spawner(spawn);
    let h = harness_for(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 1600));
    one(ui, "New exam").invoke_accessible_default_action();
    one(ui, "Import from Remark export…").invoke_accessible_default_action();
    let rm = ui.global::<RemarkVM>();

    one(ui, "Remark workbooks").set_accessible_value(rmx.to_str().unwrap());
    one(ui, "Read export").invoke_accessible_default_action();
    assert!(rm.get_refused() && !rm.get_has());
    assert!(
        rm.get_refusal().contains("never opened"),
        "{}",
        rm.get_refusal()
    );
    assert!(rm.get_remedy().contains(".xlsx"), "{}", rm.get_remedy());
    assert_eq!(
        one(ui, "Use this import").accessible_enabled(),
        Some(false),
        "nothing to use"
    );

    one(ui, "Remark workbooks").set_accessible_value(joined(&books));
    one(ui, "Read export").invoke_accessible_default_action();
    assert!(rm.get_refused());
    let why = rm.get_refusal().to_string();
    assert!(
        why.contains("version A") && why.contains("questions 2, 3"),
        "{why}"
    );
    assert!(!why.contains("--points"), "the desktop's words: {why}");
    assert!(
        rm.get_suggestion().contains("question 2 → 0.75")
            && rm.get_suggestion().contains("question 3 → 0.75"),
        "{}",
        rm.get_suggestion()
    );
    labels_unique_and_inside(ui, "New exam · Remark refusal");
    one(ui, "Use the suggested points").invoke_accessible_default_action();
    assert!(rm.get_has() && !rm.get_refused(), "{}", message(ui));
    let given = rm
        .get_facts()
        .iter()
        .find(|f| f.label == "Points given")
        .map(|f| f.value.to_string())
        .expect("the points given are named");
    assert!(given.contains("given by you"), "{given}");
    let reproduced = rm
        .get_facts()
        .iter()
        .find(|f| f.label == "Remark totals")
        .map(|f| f.value.to_string())
        .expect("totals");
    assert!(reproduced.starts_with("6 of 6 rows"), "{reproduced}");
}

/// §3.5 in Setup: an exam graded with an inferred key takes its key from Remark's export. The
/// import is shown for review (nothing changes); an export whose points differ from the
/// graded exam's is shown as not applicable, with why (a graded exam's points are fixed with
/// its stored form map) and what to do; the export with the exam's points, applied, replaces
/// the key (a new key version, no longer inferred, rescored, audited); the learning objectives
/// are listed, not applied (no core command replaces a graded exam's CLO mapping), and the
/// message says so.
#[test]
fn req_a11_remark_import_in_setup_replaces_the_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 12,
        ..synth::QUIZ
    };
    let built = synth::build(&spec, dir.path());
    let weighted = remark::write_books(&spec, &dir.path().join("remark-weighted"));
    let books = remark::write_books_with(&spec, &dir.path().join("remark"), true);
    let app = AppBackend::project(&built.exam_dir, "principal:a11-instructor").expect("open");
    let exam = app.default_exam().expect("exam");
    let h = harness_for(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 1600));
    let key = |h: &Harness| {
        h.app
            .borrow()
            .with_project(&exam, |p| {
                let k = p.session.key.as_ref().expect("key");
                (k.version, k.inferred)
            })
            .expect("project")
    };
    let (before, inferred) = key(&h);
    assert!(inferred, "graded with an inferred key");
    tab(ui, "Setup").invoke_accessible_default_action();
    let rm = ui.global::<RemarkVM>();

    // other points than the graded exam's: shown, not applicable, with why and what to do
    one(ui, "Import from Remark export…").invoke_accessible_default_action();
    one(ui, "Remark workbooks").set_accessible_value(joined(&weighted[..2]));
    read_export_with_suggested_points(ui);
    assert!(rm.get_setup_open() && rm.get_has(), "{}", message(ui));
    assert!(!rm.get_can_apply());
    let why = rm.get_applies().to_string();
    assert!(
        why.starts_with("Not applicable here") && why.contains("New exam"),
        "{why}"
    );
    assert_eq!(
        one(ui, "Apply key and points").accessible_enabled(),
        Some(false)
    );
    labels_unique_and_inside(ui, "Setup · Remark import not applicable");

    // another exam's export (none of its students is this exam's): not applicable
    let other = remark::write_books_with(&MIDTERM, &dir.path().join("remark-other"), true);
    one(ui, "Import from Remark export…").invoke_accessible_default_action();
    one(ui, "Remark workbooks").set_accessible_value(joined(&other[..2]));
    read_export_with_suggested_points(ui);
    assert!(rm.get_has() && !rm.get_can_apply(), "{}", rm.get_applies());
    assert!(
        rm.get_applies().contains("none of the export's"),
        "{}",
        rm.get_applies()
    );

    // the export with the exam's points
    one(ui, "Import from Remark export…").invoke_accessible_default_action();
    one(ui, "Remark workbooks").set_accessible_value(joined(&books[..2]));
    read_export_with_suggested_points(ui);
    assert!(rm.get_has() && rm.get_can_apply(), "{}", rm.get_applies());
    assert_eq!(rm.get_context(), 2);
    assert!(
        rm.get_applies().contains("key version"),
        "{}",
        rm.get_applies()
    );
    assert_eq!(key(&h), (before, true), "nothing changes before Apply");
    labels_unique_and_inside(ui, "Setup · Remark import");
    // back to the matrix and to the waiting import
    one(ui, "Key and CLO mapping").invoke_accessible_default_action();
    assert!(!rm.get_setup_open());
    one(ui, "Remark import · review").invoke_accessible_default_action();
    assert!(rm.get_setup_open());
    one(ui, "Apply key and points").invoke_accessible_default_action();
    let msg = message(ui);
    assert!(
        msg.contains("in force from the Remark import") && msg.contains("not applied"),
        "{msg}"
    );
    assert_eq!(
        key(&h),
        (before + 1, false),
        "a new key version, not inferred"
    );
    let keys = h
        .app
        .borrow()
        .with_project(&exam, |p| match &p.session.definition.key {
            omr_app::exam::KeySource::Provided { forms } => forms.clone(),
            k => panic!("key {k:?}"),
        })
        .expect("project");
    for (fi, k) in keys.iter().enumerate() {
        for q in 0..synth::QUESTIONS {
            assert_eq!(
                k.answers[usize::from(q)],
                char::from(b'A' + synth::key(&spec, fi, q)).to_string()
            );
        }
    }
    let label = h.app.borrow().results(&exam).expect("results").key_label;
    assert!(!label.to_uppercase().contains("INFERRED"), "{label}");
    assert!(
        !rm.get_setup_open() && !rm.get_has(),
        "the import is used up"
    );
}

/// §23.7 in Results: the parallel run beside Remark's workbooks of the same sheets, under the
/// Restricted / Q-08 banner: the counts equal omr-app's own comparison; the settled difference
/// Remark read otherwise is listed first with its bubble-row crop; the identity differences
/// (a booklet with no row, a row with no student number), the key sheet Remark graded as a
/// student and another section's workbook are named; the report is written where asked, every
/// file with the banner.
#[test]
fn req_a11_parallel_run_in_results_shows_the_differences_with_crops() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    // Remark graded the same sheets with the exam's own points (1 per question)
    let books = remark::write_books_with(&MIDTERM, &dir.path().join("remark"), true);
    let app = AppBackend::project(&built.exam_dir, "principal:a11-instructor").expect("open");
    let exam = app.default_exam().expect("exam");
    let h = harness_for(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1440, 2400));
    tab(ui, "Results").invoke_accessible_default_action();
    one(ui, "Compare with Remark export…").invoke_accessible_default_action();
    let rm = ui.global::<RemarkVM>();
    assert!(rm.get_cmp_bar());
    one(ui, "Remark workbooks to compare").set_accessible_value(joined(&books));
    one(ui, "Compare").invoke_accessible_default_action();
    assert!(
        rm.get_cmp_open() && !rm.get_cmp_refused(),
        "{}",
        message(ui)
    );
    let banner = with_prefix(ui, "Restricted: RESTRICTED");
    assert_eq!(banner.len(), 1, "the Restricted banner");
    assert!(banner[0].0.contains("Q-08"), "{}", banner[0].0);

    // the counts are omr-app's own (an independent run over the same project)
    let oracle = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            let wbs = omr_app::remark::read_workbook_files(&books).expect("workbooks");
            p.compare_remark(&wbs, &omr_app::parallel::CompareOptions::default())
                .expect("compare")
                .counts
        })
        .expect("project");
    let tiles: BTreeMap<String, String> = rm
        .get_cmp_tiles()
        .iter()
        .map(|t| (t.label.to_string(), t.value.to_string()))
        .collect();
    assert_eq!(
        tiles["TO ADJUDICATE"],
        (oracle.differ + oracle.differ_pending).to_string()
    );
    assert_eq!(tiles["QUESTIONS EQUAL"], oracle.agree.to_string());
    assert_eq!(tiles["KEY SHEETS"], "1");
    assert!(oracle.differ >= 1, "{oracle:?}");

    // the settled difference first, with its crop, then the pending ones
    let q = remark::misread_question(&MIDTERM) + 1;
    let diffs: Vec<_> = rm.get_diffs().iter().collect();
    assert_eq!(
        diffs.len(),
        oracle.differ + oracle.differ_pending + oracle.agree_pending
    );
    let first = &diffs[0];
    assert!(
        first.status.starts_with("differs · ours"),
        "{}",
        first.status
    );
    assert_eq!(first.q, format!("Q{q:02}"));
    assert!(first.has_crop, "the difference carries its crop");
    let crop = rm.get_diff_crops().row_data(0).expect("crop");
    assert!(
        crop.size().width > 100,
        "a real bubble row, not a placeholder"
    );
    assert_eq!(
        by_label(ui, first.label.as_str()).len(),
        1,
        "the difference is one AT node: {}",
        first.label
    );
    // identity differences, the key sheet and the other section are named
    let identity: Vec<String> = with_prefix(ui, "Identity difference: ")
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert!(
        identity
            .iter()
            .any(|l| l.contains("booklet with no row in Remark")),
        "{identity:?}"
    );
    assert!(
        identity
            .iter()
            .any(|l| l.contains("Remark's row has no student number (paired by answers)")),
        "{identity:?}"
    );
    let unread = remark::error_id_sheet(&MIDTERM).expect("an ERROR row");
    assert!(
        identity
            .iter()
            .all(|l| !l.contains(&synth::roster_id(&MIDTERM, unread))),
        "student numbers masked on screen: {identity:?}"
    );
    assert_eq!(with_prefix(ui, "Key sheets: ").len(), 1);
    assert!(
        with_prefix(ui, "Workbook: ")
            .iter()
            .any(|(l, _)| l.contains("Section173") && l.contains("not compared")),
        "another section's workbook"
    );
    labels_unique_and_inside(ui, "Results · parallel run");

    // the Restricted report, where the instructor asks
    let out = dir.path().join("report");
    one(ui, "Report folder").set_accessible_value(out.to_str().unwrap());
    one(ui, "Save report").invoke_accessible_default_action();
    assert!(
        message(ui).contains("Wrote the Restricted parallel-run report"),
        "{}",
        message(ui)
    );
    for f in omr_app::parallel::REPORT_FILES {
        let text = std::fs::read_to_string(out.join(f)).expect("report file");
        assert!(text.contains("RESTRICTED"), "{f} carries the banner");
    }
    assert!(out.join("manifest.json").exists(), "signed manifest");

    // F2: the comparison alone holds nothing, and says so; queued, each settled difference
    // from an auto-accepted answer waits in Review and holds release
    assert!(
        rm.get_cmp_hold().contains("does not hold release"),
        "{}",
        rm.get_cmp_hold()
    );
    let blocking = |h: &Harness| {
        h.app
            .borrow()
            .with_project(&exam, |p| p.session.open_blocking())
            .expect("project")
    };
    let before = blocking(&h);
    one(ui, "Queue for review").invoke_accessible_default_action();
    assert!(message(ui).contains("Queued"), "{}", message(ui));
    assert_eq!(blocking(&h), before + oracle.not_queued);
    assert!(
        rm.get_cmp_hold().contains("wait in Review"),
        "{}",
        rm.get_cmp_hold()
    );
    assert!(!rm.get_cmp_can_queue(), "nothing left to queue");
    assert!(
        by_label(ui, "Queue for review").is_empty(),
        "the button is gone once queued"
    );
    let queued = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            p.session
                .review
                .items
                .iter()
                .filter(|i| {
                    i.is_open() && i.sub_reason.as_deref() == Some(omr_app::parallel::PARALLEL_SUB)
                })
                .count()
        })
        .expect("project");
    assert_eq!(queued, oracle.not_queued);
    one(ui, "Back to results").invoke_accessible_default_action();
    assert!(!rm.get_cmp_open());
    assert_eq!(ui.global::<App>().get_page(), Page::Results);
}

/// The new states in both themes at the smallest window: every control labelled once and
/// inside the window.
#[test]
fn req_a11_remark_states_fit_the_minimum_window_in_both_themes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 12,
        ..MIDTERM
    };
    let built = synth::build(&spec, dir.path());
    let books = remark::write_books(&spec, &dir.path().join("remark"));
    let ws = built.exam_dir.parent().map(Path::to_path_buf).expect("ws");
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11-instructor");
    app.set_spawner(spawn);
    let h = harness_for(app);
    let ui = &h.ui;
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let exam: ExamId = h.app.borrow().default_exam().expect("exam");
    h.shell.select_exam(&exam);
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        h.shell.navigate(Page::Exams);
        one(ui, "New exam").invoke_accessible_default_action();
        one(ui, "Import from Remark export…").invoke_accessible_default_action();
        one(ui, "Remark workbooks").set_accessible_value(joined(&books));
        one(ui, "Read export").invoke_accessible_default_action();
        labels_unique_and_inside(ui, &format!("New exam import, dark {dark}"));
        one(ui, "Back to the form").invoke_accessible_default_action();
        ui.global::<ExamsVM>().set_new_open(false);
        h.shell.navigate(Page::Setup);
        one(ui, "Import from Remark export…").invoke_accessible_default_action();
        one(ui, "Remark workbooks").set_accessible_value(joined(&books[..2]));
        one(ui, "Read export").invoke_accessible_default_action();
        labels_unique_and_inside(ui, &format!("Setup import, dark {dark}"));
        h.shell.navigate(Page::Results);
        labels_unique_and_inside(ui, &format!("Results, dark {dark}"));
        one(ui, "Compare with Remark export…").invoke_accessible_default_action();
        one(ui, "Remark workbooks to compare").set_accessible_value(joined(&books));
        one(ui, "Compare").invoke_accessible_default_action();
        assert!(ui.global::<RemarkVM>().get_cmp_open(), "{}", message(ui));
        labels_unique_and_inside(ui, &format!("Results parallel run, dark {dark}"));
        one(ui, "Back to results").invoke_accessible_default_action();
    }
}
