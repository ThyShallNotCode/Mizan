//! The window's words: text the app shows names no section of the design, no decision id, no
//! command line and no crate (the project's rule: say it in plain words), and promises nothing
//! the window does not do. Every screen is read through the accessible tree, with the status
//! message, the Setup error and the Review help, in the demo and in real exams whose core text
//! names the command line's way: a key inferred from the sheets, a key read from key sheets
//! and not confirmed yet, a probable mis-key, CLO mappings that differ between versions, a key
//! cell given what it cannot take. Synthetic data only.

mod support;

use i_slint_backend_testing::AccessibleRole;
use omr_app::ExamProject;
use shell_slint::app_backend::AppBackend;
use shell_slint::backend::{Backend, ExamId, ItemId, Lane, RemarkRequest};
use shell_slint::bind::Shell;
use shell_slint::demo::DemoBackend;
use shell_slint::ui::{App, AppWindow, Page, ReviewVM, SetupVM, Theme};
use shell_slint::words::technical;
use slint::ComponentHandle;
use slint::platform::Key;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use support::a11::{all_texts, click, one, special, type_text, with_prefix};
use support::harness::{harness, held_spawner};
use support::synthetic_exam::{self as synth, MIDTERM};

/// Every screen, by the label of its tab in the navigation rail.
const PAGES: [(Page, &str); 7] = [
    (Page::Exams, "Exams"),
    (Page::Setup, "Setup"),
    (Page::Scan, "Scan"),
    (Page::Review, "Review"),
    (Page::Results, "Results"),
    (Page::Outcomes, "Outcomes"),
    (Page::Settings, "Settings"),
];

/// Everything the window shows now: its accessible texts, the status message, the Setup error
/// and the Review help.
fn shown(ui: &AppWindow) -> Vec<String> {
    let mut texts = all_texts(ui);
    texts.push(ui.global::<App>().get_message().to_string());
    texts.push(ui.global::<SetupVM>().get_error().to_string());
    texts.push(ui.global::<ReviewVM>().get_help_text().to_string());
    texts
}

fn assert_plain(ui: &AppWindow, screen: &str) {
    for t in shown(ui) {
        if let Some(what) = technical(&t) {
            panic!("{screen} shows {what}: {t:?}");
        }
    }
}

fn assert_plain_text(t: &str, what: &str) {
    if let Some(found) = technical(t) {
        panic!("{what} shows {found}: {t:?}");
    }
}

fn message(ui: &AppWindow) -> String {
    ui.global::<App>().get_message().to_string()
}

/// Opens the screen `name` with a click on its tab in the navigation rail (a tab may carry a
/// count after its name: "Review, 3 open").
fn open(ui: &AppWindow, page: Page, name: &str) {
    let tabs: Vec<_> = with_prefix(ui, name)
        .into_iter()
        .filter(|(l, e)| {
            e.accessible_role() == Some(AccessibleRole::Tab)
                && (l == name || l.starts_with(&format!("{name},")))
        })
        .collect();
    assert_eq!(
        tabs.len(),
        1,
        "one tab {name:?}: {:?}",
        tabs.iter().map(|t| &t.0).collect::<Vec<_>>()
    );
    click(ui, &tabs[0].1);
    assert_eq!(ui.global::<App>().get_page(), page, "{name}");
}

/// Every screen, and every lane of Review.
fn walk(shell: &Shell, ui: &AppWindow, exam: &str) {
    for (p, name) in PAGES {
        open(ui, p, name);
        assert_plain(ui, &format!("{exam}: {p:?}"));
    }
    open(ui, Page::Review, "Review");
    for lane in Lane::ALL {
        shell.set_lane(lane);
        assert_plain(ui, &format!("{exam}: Review, {lane:?}"));
    }
}

/// The demo, on every screen and after the buttons only the demo has: the rail's mark, the
/// status line and the demo's own messages say how to open real exams in words, never as a
/// command line, and never point to a step the demo does not offer.
#[test]
fn the_demo_says_everything_in_plain_words() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let demo = Rc::new(RefCell::new(DemoBackend::new()));
    let dynamic: Rc<RefCell<dyn Backend>> = demo.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1100.0));
    walk(&shell, &ui, "demo");
    let engine = ui.global::<App>().get_engine().to_string();
    assert!(engine.starts_with("Mizan "), "{engine}");
    // the demo's own buttons answer in words
    for (page, name, button) in [
        (Page::Exams, "Exams", "New exam"),
        (Page::Scan, "Scan", "Rescan pull list"),
        (Page::Review, "Review", "Filter the queue"),
    ] {
        open(&ui, page, name);
        click(&ui, &one(&ui, button));
        assert_plain(&ui, &format!("demo: {button}"));
    }
    // what the demo refuses says how to leave it, in words: it cannot open an exams folder
    let exam = ExamId(shell_slint::demo::CS211.into());
    let mut d = demo.borrow_mut();
    let file = Path::new("x.csv");
    let refusals = [
        d.release(&exam).unwrap_err(),
        d.export(&exam).unwrap_err(),
        d.export_blackboard(&exam, None).unwrap_err(),
        d.set_operator("Dr. A").unwrap_err(),
        d.add_scans(&exam, &[]).unwrap_err(),
        d.open_workspace(Path::new("x")).unwrap_err(),
        d.replace_key(&exam, file).unwrap_err(),
        d.approve_mappings(&exam).unwrap_err(),
        d.replace_clos(&exam, file).unwrap_err(),
        d.read_key_sheets(&exam, &[]).unwrap_err(),
        d.confirm_key_sheets(&exam, None, &[]).unwrap_err(),
        d.read_remark_import(Some(&exam), &RemarkRequest::default())
            .unwrap_err(),
        d.apply_remark_import(&exam).unwrap_err(),
        d.compare_remark(&exam, &RemarkRequest::default())
            .unwrap_err(),
        d.queue_parallel_differences(&exam).unwrap_err(),
        d.save_parallel_run(&exam, Path::new("x")).unwrap_err(),
    ];
    for e in refusals {
        let e = e.to_string();
        assert_plain_text(&e, "a demo refusal");
        assert!(e.contains("start Mizan again"), "{e}");
    }
    assert_plain_text(
        &d.create_exam_blocker().unwrap_or_default(),
        "the demo's New exam",
    );
}

/// An exam graded without a key (the key is inferred): the release reason, the Setup findings
/// and the status line name Setup's Replace key…, not the command that replaces a key.
#[test]
fn an_inferred_key_is_said_in_plain_words() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&synth::BARE, dir.path());
    let h = harness(AppBackend::project(&built.project, "principal:words").expect("open"));
    h.ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1100.0));
    walk(&h.shell, &h.ui, "inferred key");
    let r = h.app.borrow().results(&h.exam).expect("results");
    let why = r.release_blocked.expect("an inferred key blocks release");
    assert!(why.contains("Replace key…"), "{why}");
    assert_plain_text(&why, "the release reason");
}

/// An exam graded with the instructor's key sheets and not confirmed yet: the Setup findings,
/// the key-sheet summary, a key typed into the grid and a batch of scans added say where in
/// this window the key is confirmed (Setup, Key from answer sheets), never the command line.
#[test]
fn a_key_from_key_sheets_is_said_in_plain_words() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 12,
        ..synth::FINAL
    };
    let built = synth::build_with_key_sheets(&spec, dir.path());
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::project(&built.exam_dir, "principal:words").expect("open");
    app.set_spawner(spawn);
    let h = harness(app);
    let ui = &h.ui;
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 2400.0));
    walk(&h.shell, ui, "key sheets");
    open(ui, Page::Setup, "Setup");
    let lints = h.app.borrow().exam_setup(&h.exam).expect("setup").lints;
    assert!(
        lints
            .iter()
            .any(|l| l.message.contains("Key from answer sheets")),
        "{lints:?}"
    );
    let sum = h.app.borrow().key_sheets(&h.exam).expect("key sheets");
    for (v, from) in &sum.other_versions {
        assert_plain_text(from, &format!("version {v}'s key"));
    }
    // the grid, with its Checks card, in place of the key-sheet summary Setup opens on
    click(ui, &one(ui, "Key and CLO mapping"));
    assert_plain(ui, "Setup's grid while the key sheets wait");
    // a key cell clicked (it steps to the next letter) and typed into while the key waits:
    // refused where it is confirmed, without the typing hint of a refused entry
    let cell = one(ui, "Q01 version A key");
    click(ui, &cell);
    for m in [message(ui), {
        type_text(ui, "b");
        message(ui)
    }] {
        assert!(m.contains("Key from answer sheets"), "{m}");
        assert!(!m.contains("Type an option letter"), "{m}");
        assert_plain_text(&m, "a key typed while the key sheets wait");
    }
    // a batch of scans is refused at once, in words, before the exam is handed to a worker
    let scans = dir.path().join("more");
    std::fs::create_dir_all(&scans).expect("dir");
    let e = h
        .app
        .borrow_mut()
        .add_scans(&h.exam, &[scans])
        .expect_err("the key waits for its confirmation");
    let e = e.to_string();
    assert!(
        e.contains("waits for your confirmation") && e.contains("Key from answer sheets"),
        "{e}"
    );
    assert_plain_text(&e, "scans added while the key sheets wait");
    assert!(h.app.borrow().exam_setup(&h.exam).is_some(), "still open");
}

/// A key cell takes one key press: an option letter, * or Delete. What it cannot take (a
/// letter the exam does not use, an AND key) is refused with that, and two accepted answers
/// are pointed to a key file: never the core's remedy, which writes a key file's entries.
#[test]
fn a_key_cell_refusal_says_what_the_cell_takes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    let mut app = AppBackend::project(&built.project, "principal:words").expect("open");
    let exam = app.default_exam().expect("exam");
    for typed in ["Z", "+"] {
        let e = app
            .set_key(&exam, 0, 1, typed)
            .expect_err("a key cell takes one option letter")
            .to_string();
        assert!(
            e.contains("Type an option letter") && e.contains("Replace key…"),
            "{e}"
        );
        for promise in ["several accepted letters", "write A|C", "write one letter"] {
            assert!(!e.contains(promise), "{typed}: {e}");
        }
        assert_plain_text(&e, "a refused key cell");
    }
}

/// Keeping the key of a probable mis-key needs a reason and a note whatever the reason (the
/// core refuses it otherwise): in the window the item asks why the key is kept, waits for a
/// reason and then for the note, and only then acknowledges; its detail names the decisions
/// in words.
#[test]
fn keeping_a_probable_mis_key_asks_for_a_reason_and_a_note() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    // version A's key is made wrong on the question where a distractor most tells the weaker
    // half of the class from the stronger: a probable mis-key (SCO-001)
    let (q, d) = miskeyed(&MIDTERM);
    let mut key = "question,A,B\n".to_string();
    for k in 0..synth::QUESTIONS {
        let a = if k == q {
            d
        } else {
            synth::key(&MIDTERM, 0, k)
        };
        key.push_str(&format!(
            "{},{},{}\n",
            k + 1,
            char::from(b'A' + a),
            char::from(b'A' + synth::key(&MIDTERM, 1, k))
        ));
    }
    let key_file = dir.path().join("miskey.csv");
    std::fs::write(&key_file, key).expect("write");
    {
        let mut p = ExamProject::open(&built.project, "principal:words").expect("open");
        let kf = omr_app::define::read_key_path_full(&key_file, &p.session.definition)
            .expect("key file");
        p.replace_key_points(kf.forms, None, None)
            .expect("key replaced");
        p.judge_flags().expect("flags judged");
    }
    let app = AppBackend::project(&built.project, "principal:words").expect("open");
    let exam = app.default_exam().expect("exam");
    let flagged: Vec<u64> = app
        .with_project(&exam, |p| {
            p.session
                .review
                .items
                .iter()
                .filter(|i| i.flag.is_some() && i.is_open())
                .map(|i| i.id)
                .collect()
        })
        .expect("project");
    assert!(!flagged.is_empty(), "the planted mis-key is flagged");
    for &id in &flagged {
        let it = app.review_item(&exam, ItemId(id)).expect("the item");
        assert!(!it.reasons.is_empty(), "{}", it.code);
        assert_eq!(
            it.reasons_needing_note, it.reasons,
            "every reason to keep the key needs a note"
        );
        assert_plain_text(&it.summary, "a probable mis-key");
        if let Some(n) = &it.notice {
            for (k, x) in &n.lines {
                assert_plain_text(x, k);
            }
        }
    }
    // in the window
    let h = harness(app);
    let ui = &h.ui;
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1100.0));
    let id = ItemId(flagged[0]);
    let lane = Lane::ALL
        .into_iter()
        .find(|&l| {
            h.app
                .borrow()
                .review_queue(&h.exam, l)
                .groups
                .iter()
                .any(|g| g.items.iter().any(|i| i.id == id))
        })
        .expect("the flagged item is queued");
    open(ui, Page::Review, "Review");
    h.shell.set_lane(lane);
    h.shell.select_item(Some(id));
    assert!(
        all_texts(ui).iter().any(|t| t == "WHY KEEP THE KEY?"),
        "the item asks why the key is kept"
    );
    assert_eq!(one(ui, "Choose a reason").accessible_enabled(), Some(false));
    let chips = with_prefix(ui, "Reason: ");
    assert!(!chips.is_empty(), "a reason to choose");
    let reason = chips[0].0.clone();
    // a reason chosen and taken back (a chip clicked twice): Enter asks for one
    click(ui, &one(ui, &reason));
    click(ui, &one(ui, &reason));
    special(ui, Key::Return);
    assert!(
        message(ui).contains("Keeping the key needs a reason and a note"),
        "{}",
        message(ui)
    );
    click(ui, &one(ui, &reason));
    assert_eq!(
        one(ui, "Write why in the note").accessible_enabled(),
        Some(false)
    );
    special(ui, Key::Return);
    assert!(message(ui).contains("needs a note"), "{}", message(ui));
    one(ui, "Reviewer note (personal data, PDPL)")
        .set_accessible_value("checked with the instructor: the key is right");
    click(ui, &one(ui, "Acknowledge"));
    let open_now = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            p.session.review.item(id.0).map(|i| i.is_open())
        })
        .expect("project");
    assert_eq!(open_now, Some(false), "{}", message(ui));
    let last = h
        .app
        .borrow()
        .review_history(&h.exam)
        .pop()
        .expect("recorded");
    assert!(last.reason.is_some(), "{last:?}");
    assert_plain(ui, "Review after keeping the key");
}

/// The question of version A (0-based, not the planted one) and the distractor whose choosers
/// are most the weaker half of the class: keyed so, the key discriminates against the class.
fn miskeyed(spec: &synth::Spec) -> (u16, u8) {
    let sheets: Vec<synth::SheetTruth> = (0..spec.sheets)
        .map(|k| synth::sheet_truth(spec, k))
        .collect();
    let score = |t: &synth::SheetTruth| {
        (0..synth::QUESTIONS)
            .filter(|&q| t.answers[usize::from(q)] == Some(synth::key(spec, t.form, q)))
            .count()
    };
    let mut a: Vec<(usize, &synth::SheetTruth)> = sheets
        .iter()
        .filter(|t| t.form == 0)
        .map(|t| (score(t), t))
        .collect();
    a.sort_by_key(|(s, _)| *s);
    let half = a.len() / 2;
    let mut best = (0, 0, i64::MIN);
    for q in (0..synth::QUESTIONS).filter(|&q| q != synth::MISKEY_Q) {
        for d in (0..synth::OPTIONS).filter(|&d| d != synth::key(spec, 0, q)) {
            let chose = |t: &synth::SheetTruth| i64::from(t.answers[usize::from(q)] == Some(d));
            let low: i64 = a[..half].iter().map(|(_, t)| chose(t)).sum();
            let high: i64 = a[half..].iter().map(|(_, t)| chose(t)).sum();
            if low - high > best.2 {
                best = (q, d, low - high);
            }
        }
    }
    (best.0, best.1)
}

/// CLO mappings that differ between versions (one cell stands for several rows): a weight or a
/// note typed into the grid is refused in words, with Setup's Replace CLOs as the way.
#[test]
fn a_per_version_mapping_cell_is_refused_in_plain_words() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(
        &synth::Spec {
            sheets: 12,
            ..MIDTERM
        },
        dir.path(),
    );
    let clo = dir.path().join("clo-per-version.csv");
    std::fs::write(
        &clo,
        "clo,domain,title,plo,questions,weight,note,form\n\
         CLO1,K,Describe the basic data structures,K1,1-15,1,,A\n\
         CLO1,K,Describe the basic data structures,K1,16-30,1,,B\n\
         CLO2,S,Analyse the cost of an algorithm,S1,16-30,1,,A\n\
         CLO2,S,Analyse the cost of an algorithm,S1,1-15,1,,B\n",
    )
    .expect("write");
    let mut app = AppBackend::project(&built.project, "principal:words").expect("open");
    let exam = app.default_exam().expect("exam");
    app.replace_clos(&exam, &clo).expect("per-version rows");
    assert!(
        app.with_project(&exam, |p| p.session.definition.has_form_mapping())
            .expect("project")
    );
    let h = harness(app);
    let ui = &h.ui;
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1100.0));
    open(ui, Page::Setup, "Setup");
    let weights = |h: &support::harness::Harness| {
        h.app
            .borrow()
            .exam_setup(&h.exam)
            .expect("setup")
            .mapping
            .iter()
            .map(|r| r.cells.iter().map(|c| (c.weight, c.note.clone())).collect())
            .collect::<Vec<Vec<_>>>()
    };
    let before = weights(&h);
    // a weight typed into a cell
    click(ui, &one(ui, "Q01 CLO1 weight"));
    type_text(ui, "0.5");
    special(ui, Key::Return);
    // a note given to the cell in the editor beside the grid
    let note = with_prefix(ui, "Note of Q");
    assert_eq!(note.len(), 1, "the cell's note");
    let typed = message(ui);
    note[0].1.set_accessible_value("asked twice");
    for m in [typed, message(ui)] {
        assert!(
            m.contains("Replace CLOs") && !m.contains("weight above 0"),
            "{m}"
        );
        assert_plain_text(&m, "a mapping cell edit");
    }
    assert_eq!(weights(&h), before, "nothing changed");
    assert_plain(ui, "Setup after a refused cell");
}

/// Every text written into the window's markup (`ui/**/*.slint`: labels, placeholders,
/// descriptions, titles) is in plain words too, whatever state shows it.
#[test]
fn the_markup_says_everything_in_plain_words() {
    fn slint_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("ui folder") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                slint_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "slint") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    slint_files(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"),
        &mut files,
    );
    assert!(files.len() > 5, "{files:?}");
    for f in files {
        let text = std::fs::read_to_string(&f).expect("read");
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            // the string literals of the line (a literal may hold an escaped quote)
            let mut rest = code;
            while let Some(open) = rest.find('"') {
                let body = &rest[open + 1..];
                let mut end = None;
                let mut escaped = false;
                for (i, c) in body.char_indices() {
                    match c {
                        '\\' if !escaped => escaped = true,
                        '"' if !escaped => {
                            end = Some(i);
                            break;
                        }
                        _ => escaped = false,
                    }
                }
                let Some(end) = end else {
                    break;
                };
                let literal = &body[..end];
                if let Some(what) = technical(literal) {
                    panic!("{}:{} shows {what}: {literal:?}", f.display(), n + 1);
                }
                rest = &body[end + 1..];
            }
        }
    }
}
