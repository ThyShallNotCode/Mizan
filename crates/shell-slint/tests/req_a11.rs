//! A11 acceptance tests (IMPLEMENTATION.md §1, ADR-001 S7): the review flow is driven headless
//! through accessible labels and the keyboard, every screen renders in both themes, and every
//! interactive control carries a unique accessible label (DESIGN §8, §10).
//!
//! Each test runs on its own thread with its own Slint platform (the testing backend, or the
//! software-renderer platform for the render test).

mod support;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use shell_slint::backend::{Backend, Decision, ExamId, Lane, ReviewAction};
use shell_slint::bind::Shell;
use shell_slint::demo::{CS211, DemoBackend, MATH101};
use shell_slint::ui::{
    App, AppWindow, ExamsVM, MarkState, OutcomesVM, Page, ReviewVM, ScanVM, SetupVM, Theme,
};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use support::a11::{by_label, chord, key, one, selected_queue_row, special, tab, with_prefix};

struct Harness {
    ui: AppWindow,
    demo: Rc<RefCell<DemoBackend>>,
    shell: Rc<Shell>,
}

fn harness() -> Harness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let demo = Rc::new(RefCell::new(DemoBackend::new()));
    let dynamic: Rc<RefCell<dyn Backend>> = demo.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    Harness { ui, demo, shell }
}

fn cs211() -> ExamId {
    ExamId(CS211.into())
}

fn last_action(h: &Harness) -> (ReviewAction, String, bool) {
    let hist = h.demo.borrow().review_history(&cs211());
    let r = hist.last().expect("a recorded decision").clone();
    (r.action, r.serial, r.undone)
}

// ------------------------------------------------------------------------------------ flow

#[test]
fn req_a11_review_flow_through_accessible_labels() {
    let h = harness();
    let ui = &h.ui;
    assert_eq!(ui.global::<App>().get_page(), Page::Exams);

    // navigate to Review by its accessible label (a tab with the open count)
    let review = tab(ui, "Review, 30 open");
    assert_eq!(review.accessible_item_selected(), Some(false));
    review.invoke_accessible_default_action();
    assert_eq!(ui.global::<App>().get_page(), Page::Review);
    assert_eq!(
        tab(ui, "Review, 30 open").accessible_item_selected(),
        Some(true)
    );

    // read the first item: the selected queue row, the AccessKit text summary and the suggestion
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0412, MRK-001");
    let summary = with_prefix(ui, "Item summary: ");
    assert_eq!(summary.len(), 1);
    assert_eq!(
        summary[0].0,
        "Item summary: Question 17, form B. B filled 0.82, D light 0.31. Suggestion B, engine."
    );
    let b = one(ui, "B, suggested");
    assert_eq!(b.accessible_role(), Some(AccessibleRole::Button));
    assert_eq!(b.accessible_description().as_deref(), Some("B"));
    assert_eq!(b.accessible_checked(), Some(false));
    one(ui, "D: ambiguous"); // the overlay of the light second mark
    one(ui, "B: filled");

    // choose an option by its label: the chip becomes checked, Accept names the choice
    let d = one(ui, "D");
    d.invoke_accessible_default_action();
    assert_eq!(one(ui, "D").accessible_checked(), Some(true));
    assert_eq!(h.shell.pending(), Some(Decision::Option(3)));

    // press Accept
    let accept = one(ui, "Accept D");
    assert_eq!(accept.accessible_description().as_deref(), Some("Enter"));
    accept.invoke_accessible_default_action();

    // the backend recorded the decision ...
    let (action, serial, undone) = last_action(&h);
    assert_eq!(
        action,
        ReviewAction::Resolve {
            decision: Decision::Option(3),
            accepted_suggestion: false,
            note: String::new(),
        }
    );
    assert_eq!((serial.as_str(), undone), ("0412", false));
    // ... and the queue advanced to the next item of the group
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");
    assert!(
        by_label(ui, "Q17 Form B, booklet 0412, MRK-001").is_empty(),
        "resolved item left the queue"
    );
    assert_eq!(
        tab(ui, "Review, 29 open").accessible_item_selected(),
        Some(true)
    );
    assert_eq!(ui.global::<ReviewVM>().get_resolved(), 129);

    // accept the suggestion as is (no choice): recorded as an accepted suggestion
    one(ui, "Accept suggestion").invoke_accessible_default_action();
    let (action, serial, _) = last_action(&h);
    assert_eq!(serial, "0377");
    assert!(matches!(
        action,
        ReviewAction::Resolve {
            decision: Decision::Option(1),
            accepted_suggestion: true,
            ..
        }
    ));

    // undo through its button re-opens the item (a compensating event)
    one(ui, "Undo last decision (Ctrl+Z)").invoke_accessible_default_action();
    assert!(last_action(&h).2, "the last event is marked undone");
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");

    // defer and escalate are recorded and advance
    one(ui, "Defer").invoke_accessible_default_action();
    assert_eq!(last_action(&h).0, ReviewAction::Defer);
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0450, MRK-001");
    one(ui, "Escalate to instructor").invoke_accessible_default_action();
    assert_eq!(last_action(&h).0, ReviewAction::Escalate);

    // the special marks are chips too
    one(ui, "Blank").invoke_accessible_default_action();
    assert_eq!(h.shell.pending(), Some(Decision::Blank));
    assert_eq!(one(ui, "Blank").accessible_checked(), Some(true));

    // a disabled control exposes its state and ignores its default action: blind verify
    // items have no suggestion, so Accept stays disabled until something is chosen
    let blind = h
        .demo
        .borrow()
        .review_queue(&cs211(), Lane::Marks)
        .groups
        .iter()
        .find(|g| g.code == "VERIFY")
        .map(|g| g.items[0].id)
        .unwrap();
    h.shell.select_item(Some(blind));
    let accept = one(ui, "Choose what is marked");
    assert_eq!(accept.accessible_enabled(), Some(false));
    let n = h.demo.borrow().review_history(&cs211()).len();
    accept.invoke_accessible_default_action();
    assert_eq!(h.demo.borrow().review_history(&cs211()).len(), n);
    assert!(
        by_label(ui, "B, suggested").is_empty(),
        "blind items show no suggestion"
    );
    assert!(
        with_prefix(ui, "B: ").is_empty(),
        "blind items show no overlays"
    );
    // ... and the engine's per-bubble reading never reaches the frame (§13.4)
    let bubbles = ui.global::<ReviewVM>().get_bubbles();
    assert!(bubbles.row_count() > 0);
    for b in bubbles.iter() {
        assert_eq!(b.fill, 0.0, "blind bubble {} carries a fill", b.label);
        assert!(b.fill_text.is_empty());
        assert_eq!(b.mark, MarkState::Blank);
        assert!(!b.overlay);
    }
    assert_eq!(ui.global::<ReviewVM>().get_other_fills().row_count(), 0);
}

#[test]
fn req_a11_review_keyboard_path_with_physical_keys() {
    let h = harness();
    let ui = &h.ui;

    // Ctrl+4 jumps to Review (global keys reach the window's focus scope)
    chord(ui, Key::Control, "4");
    assert_eq!(ui.global::<App>().get_page(), Page::Review);
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0412, MRK-001");

    // a letter chooses, Enter commits
    key(ui, "d");
    assert_eq!(h.shell.pending(), Some(Decision::Option(3)));
    special(ui, Key::Return);
    let (action, serial, _) = last_action(&h);
    assert_eq!(serial, "0412");
    assert!(matches!(
        action,
        ReviewAction::Resolve {
            decision: Decision::Option(3),
            ..
        }
    ));
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");

    // a digit chooses by position: 2 = B
    key(ui, "2");
    assert_eq!(h.shell.pending(), Some(Decision::Option(1)));
    // typed Arabic option letter (abjad): ج = C
    key(ui, "ج");
    assert_eq!(h.shell.pending(), Some(Decision::Option(2)));
    // the A position under the Arabic (101) layout produces ش: still A
    key(ui, "ش");
    assert_eq!(h.shell.pending(), Some(Decision::Option(0)));
    // Arabic-Indic digit ٥ = E
    key(ui, "٥");
    assert_eq!(h.shell.pending(), Some(Decision::Option(4)));
    special(ui, Key::Return);
    let (action, serial, _) = last_action(&h);
    assert_eq!(serial, "0377");
    assert!(matches!(
        action,
        ReviewAction::Resolve {
            decision: Decision::Option(4),
            accepted_suggestion: false,
            ..
        }
    ));

    // Ctrl+Z undoes; Ctrl+Z under the Arabic layout (ئ on the Z position) as well
    chord(ui, Key::Control, "z");
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");
    assert!(last_action(&h).2);

    // special marks: Backspace = blank, Ctrl+M = multi, Ctrl+U = unreadable (ة on M, Arabic)
    special(ui, Key::Backspace);
    assert_eq!(h.shell.pending(), Some(Decision::Blank));
    chord(ui, Key::Control, "m");
    assert_eq!(h.shell.pending(), Some(Decision::Multi));
    chord(ui, Key::Control, "u");
    assert_eq!(h.shell.pending(), Some(Decision::Unreadable));
    chord(ui, Key::Control, "ة");
    assert_eq!(h.shell.pending(), Some(Decision::Multi));

    // arrows move through the queue without deciding
    let before = h.demo.borrow().review_history(&cs211()).len();
    special(ui, Key::DownArrow);
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0450, MRK-001");
    special(ui, Key::UpArrow);
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");
    assert_eq!(h.demo.borrow().review_history(&cs211()).len(), before);

    // Shift+Enter applies the suggestion to the whole group (3 → all closed)
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: char::from(Key::Shift).to_string().into(),
    });
    special(ui, Key::Return);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: char::from(Key::Shift).to_string().into(),
    });
    assert!(matches!(
        last_action(&h).0,
        ReviewAction::ApplyToGroup { .. }
    ));
    assert_eq!(selected_queue_row(ui), "Q26 Form B, booklet 0603, MRK-001");

    // Ctrl+D defers (ي is D under the Arabic layout)
    chord(ui, Key::Control, "ي");
    assert_eq!(last_action(&h).0, ReviewAction::Defer);

    // single-key shortcuts can be switched off (WCAG 2.1.4): letters stop working, Enter does not
    ui.global::<App>().invoke_set_single_key(false);
    let pending = h.shell.pending();
    key(ui, "c");
    assert_eq!(h.shell.pending(), pending);
}

#[test]
fn req_a11_identity_lane_and_candidates_by_label() {
    let h = harness();
    let ui = &h.ui;
    tab(ui, "Review, 30 open").invoke_accessible_default_action();
    one(ui, "Identity, 5").invoke_accessible_default_action();
    assert_eq!(h.shell.lane(), Lane::Identity);
    assert_eq!(
        selected_queue_row(ui),
        "Booklet 0277, writer error, IDN-004"
    );
    let top = one(ui, "Candidate 1, Sara Al-Qahtani, 0.91, suggested");
    assert_eq!(top.accessible_checked(), Some(false));
    // key 2 chooses the second candidate; Enter commits it
    key(ui, "2");
    assert_eq!(h.shell.pending(), Some(Decision::Candidate(1)));
    assert_eq!(
        one(ui, "Candidate 2, Lama Al-Shehri, 0.05").accessible_checked(),
        Some(true)
    );
    one(ui, "Accept candidate 2").invoke_accessible_default_action();
    assert!(matches!(
        last_action(&h).0,
        ReviewAction::Resolve {
            decision: Decision::Candidate(1),
            ..
        }
    ));
    // N = not on roster on the next item
    key(ui, "n");
    assert_eq!(h.shell.pending(), Some(Decision::NotOnRoster));
}

#[test]
fn req_a11_setup_matrix_edits_through_accessible_values() {
    let h = harness();
    let ui = &h.ui;
    tab(ui, "Setup").invoke_accessible_default_action();
    assert_eq!(ui.global::<App>().get_page(), Page::Setup);
    let cell = one(ui, "Q03 CLO2 weight");
    assert_eq!(cell.accessible_role(), Some(AccessibleRole::TextInput));
    cell.set_accessible_value("0,5");
    let setup = h.demo.borrow().exam_setup(&cs211()).unwrap();
    assert_eq!(setup.mapping[2].cells[1].weight, Some(0.5));
    // split rows must sum to 1: Q03 is now 1.5 and linted
    assert!(setup.lints.iter().any(|l| l.message.contains("Q03")));
    // an invalid weight is refused and explained
    one(ui, "Q03 CLO2 weight").set_accessible_value("2");
    assert_eq!(
        h.demo.borrow().exam_setup(&cs211()).unwrap().mapping[2].cells[1].weight,
        Some(0.5)
    );
    assert!(
        ui.global::<SetupVM>()
            .get_error()
            .contains("Weight must be")
    );
    // key letters by label; Arabic letters map by position
    let k = one(ui, "Q01 version A key");
    k.set_accessible_value("د");
    assert_eq!(
        h.demo.borrow().exam_setup(&cs211()).unwrap().keys[0].answers[0],
        "D"
    );
    // per-cell note (Arabic) through the selected-cell editor
    one(ui, "Q03 CLO2 weight").invoke_accessible_default_action();
    let note = with_prefix(ui, "Note of Q");
    assert_eq!(note.len(), 1);
    note[0].1.set_accessible_value("سؤال عن التعقيد");
    let any_note = h
        .demo
        .borrow()
        .exam_setup(&cs211())
        .unwrap()
        .mapping
        .iter()
        .flat_map(|r| r.cells.iter())
        .any(|c| c.note == "سؤال عن التعقيد");
    assert!(any_note);
}

// ------------------------------------------------------------------------------------ labels

const INTERACTIVE: &[AccessibleRole] = &[
    AccessibleRole::Button,
    AccessibleRole::Tab,
    AccessibleRole::Checkbox,
    AccessibleRole::Switch,
    AccessibleRole::RadioButton,
    AccessibleRole::TextInput,
    AccessibleRole::Combobox,
    AccessibleRole::Slider,
    AccessibleRole::Spinbox,
];

fn interactive_labels(ui: &AppWindow) -> Vec<(String, (i64, i64))> {
    let mut seen: BTreeMap<(String, i64, i64, i64, i64), ()> = BTreeMap::new();
    for e in ElementQuery::from_root(ui).match_descendants().find_all() {
        let Some(role) = e.accessible_role() else {
            continue;
        };
        let selectable_row =
            role == AccessibleRole::ListItem && e.accessible_item_selectable() == Some(true);
        if !INTERACTIVE.contains(&role) && !selectable_row {
            continue;
        }
        if e.computed_opacity() <= 0.0 || e.size().width <= 0.0 {
            continue;
        }
        let p = e.absolute_position();
        let s = e.size();
        let label = e
            .accessible_label()
            .map(|l| l.to_string())
            .unwrap_or_default();
        seen.insert(
            (
                label,
                (p.x * 4.0) as i64,
                (p.y * 4.0) as i64,
                (s.width * 4.0) as i64,
                (s.height * 4.0) as i64,
            ),
            (),
        );
    }
    seen.into_keys()
        .map(|(l, x, y, _, _)| (l, (x, y)))
        .collect()
}

#[test]
fn req_a11_every_control_has_a_unique_accessible_label() {
    let h = harness();
    let ui = &h.ui;
    let pages = [
        Page::Exams,
        Page::Setup,
        Page::Scan,
        Page::Review,
        Page::Results,
        Page::Outcomes,
        Page::Settings,
    ];
    let mut total = 0;
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for page in pages {
            h.shell.navigate(page);
            let labels = interactive_labels(ui);
            assert!(
                labels.len() >= 9,
                "{page:?}: only {} controls found",
                labels.len()
            );
            let mut count: BTreeMap<&str, usize> = BTreeMap::new();
            for (l, _) in &labels {
                assert!(!l.trim().is_empty(), "{page:?}: a control without a label");
                *count.entry(l.as_str()).or_default() += 1;
            }
            let dup: Vec<_> = count.iter().filter(|(_, n)| **n > 1).collect();
            assert!(dup.is_empty(), "{page:?}: duplicate labels {dup:?}");
            total += labels.len();
        }
    }
    // identity and notice frames too
    h.shell.navigate(Page::Review);
    for lane in [Lane::Identity, Lane::Capture, Lane::Scoring] {
        h.shell.set_lane(lane);
        h.shell.navigate(Page::Review);
        let labels = interactive_labels(ui);
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        for (l, _) in &labels {
            *count.entry(l.as_str()).or_default() += 1;
        }
        let dup: Vec<_> = count.iter().filter(|(_, n)| **n > 1).collect();
        assert!(dup.is_empty(), "{lane:?}: duplicate labels {dup:?}");
    }
    assert!(total > 200, "controls checked: {total}");
}

// ------------------------------------------------------------------------------------ render

#[test]
fn req_a11_every_screen_renders_in_both_themes() {
    use shell_slint::screenshot::{self, HEIGHT, SHOTS, WIDTH};
    let window = screenshot::install_platform().expect("headless platform");
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let demo = Rc::new(RefCell::new(DemoBackend::new()));
    let dynamic: Rc<RefCell<dyn Backend>> = demo.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    let mut digests = BTreeMap::new();
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for shot in SHOTS {
            screenshot::stage(&shell, &demo, shot);
            let pixels = screenshot::capture(&window);
            assert_eq!(pixels.len(), (WIDTH * HEIGHT) as usize);
            // the frame is painted: the nav rail is deep teal, the status bar follows the theme
            let rail = pixels[(600 * WIDTH + 10) as usize];
            assert!(
                rail.g > rail.r && rail.g < 80,
                "{}: rail {rail:?}",
                shot.name
            );
            let canvas = pixels[(880 * WIDTH + 700) as usize];
            let luma = (canvas.r as u32 + canvas.g as u32 + canvas.b as u32) / 3;
            assert!(
                if dark { luma < 90 } else { luma > 180 },
                "{} dark={dark}: {canvas:?}",
                shot.name
            );
            // and it is not one flat colour
            let distinct: std::collections::BTreeSet<(u8, u8, u8)> =
                pixels.iter().step_by(97).map(|p| (p.r, p.g, p.b)).collect();
            assert!(
                distinct.len() > 40,
                "{}: {} colours",
                shot.name,
                distinct.len()
            );
            let digest: u64 = pixels.iter().enumerate().fold(0u64, |h, (i, p)| {
                h.rotate_left(5) ^ (i as u64 * 31 + p.r as u64 * 7 + p.g as u64 * 3 + p.b as u64)
            });
            digests.insert((shot.name, dark), digest);
        }
    }
    let unique: std::collections::BTreeSet<u64> = digests.values().copied().collect();
    assert_eq!(
        unique.len(),
        digests.len(),
        "every screen and theme renders differently"
    );
}

#[test]
fn req_a11_notice_lane_clears_and_escape_closes_the_drawer() {
    let h = harness();
    let ui = &h.ui;
    // the exam drawer is open on start; Escape closes it
    assert!(!by_label(ui, "Close details (Esc)").is_empty());
    special(ui, Key::Escape);
    assert!(by_label(ui, "Close details (Esc)").is_empty());

    // the capture lane holds page- and batch-level notices: acknowledge both with Enter
    tab(ui, "Review, 30 open").invoke_accessible_default_action();
    one(ui, "Capture, 2").invoke_accessible_default_action();
    assert_eq!(h.shell.lane(), Lane::Capture);
    assert_eq!(one(ui, "Acknowledge").accessible_enabled(), Some(true));
    special(ui, Key::Return);
    assert!(matches!(
        last_action(&h).0,
        ReviewAction::Resolve {
            decision: Decision::Acknowledge,
            ..
        }
    ));
    special(ui, Key::Return);
    // the lane is clear: no item, no commit button, the chip shows 0
    one(ui, "Capture, 0");
    assert!(by_label(ui, "Acknowledge").is_empty());
    assert!(h.shell.current_item().is_none());
    assert_eq!(
        tab(ui, "Review, 28 open").accessible_item_selected(),
        Some(true)
    );
}

// ------------------------------------------------------------------------------------ layout

/// A frame of the shell: page, exam and, for review, the lane and n-th open item of the lane.
type Frame = (Page, &'static str, Option<(Lane, usize)>);

/// Every frame of the shell.
fn frames() -> Vec<Frame> {
    vec![
        (Page::Exams, CS211, None),
        (Page::Setup, CS211, None),
        (Page::Scan, MATH101, None),
        (Page::Scan, CS211, None),
        (Page::Review, CS211, Some((Lane::Marks, 0))),
        (Page::Review, CS211, Some((Lane::Marks, 19))), // blind verify
        (Page::Review, CS211, Some((Lane::Identity, 0))),
        (Page::Review, CS211, Some((Lane::Capture, 0))),
        (Page::Review, CS211, Some((Lane::Scoring, 0))),
        (Page::Results, CS211, None),
        (Page::Outcomes, CS211, None),
        (Page::Settings, CS211, None),
    ]
}

/// DESIGN §9: every screen works at 1440 × 900 nominal and down to 1280 × 800. The status bar
/// is fully inside the window and no control sticks out past its right edge (lists scroll
/// vertically only, so any horizontal overflow is a layout defect).
#[test]
fn req_a11_every_screen_fits_nominal_and_minimum_windows() {
    let h = harness();
    let ui = &h.ui;
    for (w, hgt) in [(1440u32, 900u32), (1280, 800)] {
        ui.window().set_size(slint::PhysicalSize::new(w, hgt));
        for (page, exam, review) in frames() {
            let exam = ExamId(exam.into());
            h.shell.select_exam(&exam);
            if let Some((lane, n)) = review {
                let q = h.demo.borrow().review_queue(&exam, lane);
                let id = q
                    .groups
                    .iter()
                    .flat_map(|g| g.items.iter())
                    .nth(n)
                    .map(|i| i.id);
                h.shell.set_lane(lane);
                h.shell.select_item(id);
            }
            h.shell.navigate(page);
            let what = format!("{page:?} {review:?} at {w}x{hgt}");
            let bars: Vec<_> = ElementHandle::find_by_element_type_name(ui, "StatusBar").collect();
            assert!(!bars.is_empty(), "{what}: no status bar");
            for b in bars {
                let (p, sz) = (b.absolute_position(), b.size());
                assert!(
                    p.y >= 0.0 && p.y + sz.height <= hgt as f32 + 0.5,
                    "{what}: status bar at y {} + {} is outside the window",
                    p.y,
                    sz.height
                );
                assert!(
                    p.x + sz.width <= w as f32 + 0.5,
                    "{what}: status bar too wide"
                );
            }
            for e in ElementQuery::from_root(ui).match_descendants().find_all() {
                let Some(role) = e.accessible_role() else {
                    continue;
                };
                if !INTERACTIVE.contains(&role) || e.computed_opacity() <= 0.0 {
                    continue;
                }
                let (p, sz) = (e.absolute_position(), e.size());
                if sz.width <= 0.0 || sz.height <= 0.0 {
                    continue;
                }
                assert!(
                    p.x + sz.width <= w as f32 + 0.5,
                    "{what}: {:?} ends at x {} past the window",
                    e.accessible_label().unwrap_or_default(),
                    p.x + sz.width
                );
            }
        }
    }
}

// ------------------------------------------------------------------------------------ keys

#[test]
fn req_a11_exams_table_and_screen_shortcuts_work_from_the_keyboard() {
    let h = harness();
    let ui = &h.ui;
    let selected_exam = |ui: &AppWindow| -> String {
        ElementQuery::from_root(ui)
            .match_descendants()
            .match_accessible_role(AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_item_selected() == Some(true))
            .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
            .find(|l| l.contains("sheets"))
            .unwrap_or_default()
    };
    assert!(selected_exam(ui).starts_with("CS 211 "));
    // activating a row selects it and puts focus on the table (one tab stop) ...
    let math = with_prefix(ui, "MATH 101 ");
    assert_eq!(math.len(), 1);
    math[0].1.invoke_accessible_default_action();
    assert!(selected_exam(ui).starts_with("MATH 101 "));
    // ... where the arrows move the selection, which is the exam the other screens show
    special(ui, Key::DownArrow);
    assert!(
        selected_exam(ui).starts_with("PHYS 110 "),
        "{}",
        selected_exam(ui)
    );
    assert!(ui.global::<App>().get_course().starts_with("PHYS 110"));
    special(ui, Key::UpArrow);
    special(ui, Key::UpArrow);
    assert!(selected_exam(ui).starts_with("CS 211 "));
    special(ui, Key::UpArrow); // stays on the first row
    assert!(selected_exam(ui).starts_with("CS 211 "));
    // Escape closes the drawer, Enter opens it again
    special(ui, Key::Escape);
    assert!(!ui.global::<ExamsVM>().get_drawer_open());
    special(ui, Key::Return);
    assert!(ui.global::<ExamsVM>().get_drawer_open());

    // the chords the buttons advertise: Ctrl+K focuses search, Ctrl+N starts a new exam
    chord(ui, Key::Control, "k");
    assert_eq!(ui.global::<ExamsVM>().get_search_request(), 1);
    chord(ui, Key::Control, "n");
    assert!(ui.global::<App>().get_message().starts_with("New exam"));

    // Scan: Ctrl+Space pauses and resumes the live batch
    h.shell.select_exam(&ExamId(MATH101.into()));
    chord(ui, Key::Control, "3");
    assert_eq!(ui.global::<App>().get_page(), Page::Scan);
    let running = ui.global::<ScanVM>().get_running();
    chord(ui, Key::Control, " ");
    assert_eq!(ui.global::<ScanVM>().get_running(), !running);
    chord(ui, Key::Control, " ");
    assert_eq!(ui.global::<ScanVM>().get_running(), running);
    chord(ui, Key::Control, "r");
    assert!(
        ui.global::<App>()
            .get_message()
            .starts_with("Rescan pull list")
    );

    // Results: Ctrl+P is the what-if preview
    h.shell.select_exam(&cs211());
    chord(ui, Key::Control, "5");
    assert_eq!(ui.global::<App>().get_page(), Page::Results);
    chord(ui, Key::Control, "p");
    assert!(ui.global::<App>().get_message().starts_with("What-if"));
}

#[test]
fn req_a11_results_rank_flags_and_readiness_actions_lead_where_they_say() {
    let h = harness();
    let ui = &h.ui;
    tab(ui, "Results").invoke_accessible_default_action();
    // the worst flag (probable mis-key, SCO-001) is the first item row (§12.2)
    let rows: Vec<String> = ElementQuery::from_root(ui)
        .match_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
        .filter(|l| l.starts_with('Q'))
        .collect();
    let first = rows
        .iter()
        .min_by_key(|l| {
            by_label(ui, l)
                .first()
                .map_or(i64::MAX, |e| e.absolute_position().y as i64)
        })
        .cloned()
        .unwrap_or_default();
    assert!(first.starts_with("Q12,"), "first item row: {first}");
    assert!(first.contains("SCO-001"));
    // the overall state is shown, and the mis-key check opens the Scoring lane
    one(ui, "Blocked · 24 ERROR open");
    let check = with_prefix(ui, "Review: 1 probable mis-key");
    assert_eq!(check.len(), 1);
    check[0].1.invoke_accessible_default_action();
    assert_eq!(ui.global::<App>().get_page(), Page::Review);
    assert_eq!(h.shell.lane(), Lane::Scoring);
}

#[test]
fn req_a11_outcomes_print_both_statistics_and_name_the_deciding_method() {
    let h = harness();
    let ui = &h.ui;
    tab(ui, "Outcomes").invoke_accessible_default_action();
    // every CLO row carries μ and π with their CIs and the verdict on the policy's method
    let rows = with_prefix(ui, "CLO");
    let clo_rows: Vec<&String> = rows
        .iter()
        .map(|(l, _)| l)
        .filter(|l| l.contains("verdict on"))
        .collect();
    assert_eq!(clo_rows.len(), 5);
    for l in &clo_rows {
        assert!(l.contains("average ") && l.contains("share at or above threshold "));
        assert!(l.contains("verdict on π"), "{l}");
    }
    assert!(
        clo_rows
            .iter()
            .any(|l| l.starts_with("CLO3 ") && l.contains("within the 95 % CI of the target"))
    );
    // §11.4 footnote: the deciding method, τ and T
    let foot = ui.global::<OutcomesVM>().get_footnote();
    assert!(foot.contains("τ = 60 %") && foot.contains("T = 70 %") && foot.contains("π"));
    assert_eq!(with_prefix(ui, "Footnote: Actual level").len(), 1);
}

/// A backend that slips: it sends the engine's reading (a suggestion, fills, other answers)
/// with blind-verify items. The shell must mask it again (§13.4), so nothing of it reaches the
/// frame, the queue sparkline or the Enter key.
struct Leaky(DemoBackend);

impl Backend for Leaky {
    fn dashboard(&self) -> shell_slint::backend::Dashboard {
        self.0.dashboard()
    }
    fn default_exam(&self) -> Option<ExamId> {
        self.0.default_exam()
    }
    fn exam_setup(&self, exam: &ExamId) -> Option<shell_slint::backend::ExamSetup> {
        self.0.exam_setup(exam)
    }
    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.0.set_key(exam, version, question, answer)
    }
    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.0.set_mapping_weight(exam, question, clo, weight)
    }
    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.0.set_mapping_note(exam, question, clo, note)
    }
    fn batch_progress(&self, exam: &ExamId) -> Option<shell_slint::backend::BatchProgress> {
        self.0.batch_progress(exam)
    }
    fn review_queue(&self, exam: &ExamId, lane: Lane) -> shell_slint::backend::ReviewQueue {
        let mut q = self.0.review_queue(exam, lane);
        for it in q.groups.iter_mut().flat_map(|g| g.items.iter_mut()) {
            if it.kind == shell_slint::backend::ReviewKind::BlindVerify {
                it.fills = vec![0.05, 0.9, 0.1, 0.0, 0.0];
            }
        }
        q
    }
    fn review_item(
        &self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
    ) -> Option<shell_slint::backend::ReviewItem> {
        let mut it = self.0.review_item(exam, item)?;
        if it.kind == shell_slint::backend::ReviewKind::BlindVerify {
            it.suggestion = Some(shell_slint::backend::Suggestion {
                decision: Decision::Option(1),
                source: shell_slint::backend::SuggestionSource::Engine,
                fill: 0.9,
                margin: 0.8,
            });
            for b in &mut it.bubbles {
                b.fill = 0.9;
                b.state = shell_slint::backend::MarkState::Filled;
            }
            it.other_fills = vec![0.8; 10];
        }
        Some(it)
    }
    fn apply(
        &mut self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
        action: ReviewAction,
    ) -> Result<shell_slint::backend::ApplyOutcome, shell_slint::backend::BackendError> {
        self.0.apply(exam, item, action)
    }
    fn undo(
        &mut self,
        exam: &ExamId,
    ) -> Result<Option<shell_slint::backend::ItemId>, shell_slint::backend::BackendError> {
        self.0.undo(exam)
    }
    fn review_history(&self, exam: &ExamId) -> Vec<shell_slint::backend::DecisionRecord> {
        self.0.review_history(exam)
    }
    fn results(&self, exam: &ExamId) -> Option<shell_slint::backend::Results> {
        self.0.results(exam)
    }
    fn outcomes(&self, exam: &ExamId) -> Option<shell_slint::backend::Outcomes> {
        self.0.outcomes(exam)
    }
}

#[test]
fn req_a11_blind_items_never_show_a_leaked_engine_reading() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let leaky = Rc::new(RefCell::new(Leaky(DemoBackend::new())));
    let dynamic: Rc<RefCell<dyn Backend>> = leaky.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    shell.navigate(Page::Review);
    let blind = leaky
        .borrow()
        .review_queue(&cs211(), Lane::Marks)
        .groups
        .iter()
        .find(|g| g.code == "VERIFY")
        .map(|g| g.items[0].id)
        .expect("a blind item");
    shell.select_item(Some(blind));
    let rv = ui.global::<ReviewVM>();
    assert!(!rv.get_has_suggestion(), "a leaked suggestion is masked");
    assert!(by_label(&ui, "B, suggested").is_empty());
    for b in rv.get_bubbles().iter() {
        assert_eq!((b.fill, b.overlay), (0.0, false), "{}", b.label);
    }
    assert_eq!(rv.get_other_fills().row_count(), 0);
    for row in rv.get_queue().iter().filter(|r| r.blind) {
        assert_eq!(row.fills.row_count(), 0, "a blind sparkline stays empty");
    }
    // Enter with nothing chosen records nothing (the leaked suggestion is never committed)
    let n = leaky.borrow().review_history(&cs211()).len();
    shell.accept();
    assert_eq!(leaky.borrow().review_history(&cs211()).len(), n);
    assert_eq!(
        one(&ui, "Choose what is marked").accessible_enabled(),
        Some(false)
    );
}

/// CR-04: demo data is marked on every screen (the rail carries "DEMO DATA" and says how to
/// open real exams), and the demo backend releases nothing.
#[test]
fn req_a11_demo_data_is_marked_on_every_screen() {
    let h = harness();
    assert!(h.ui.global::<App>().get_demo());
    for page in [
        Page::Exams,
        Page::Setup,
        Page::Scan,
        Page::Review,
        Page::Results,
        Page::Outcomes,
        Page::Settings,
    ] {
        h.shell.navigate(page);
        let mark = with_prefix(&h.ui, "Demo data: invented exams");
        assert_eq!(mark.len(), 1, "{page:?}");
        assert!(mark[0].0.contains("--workspace"), "{page:?}");
    }
    let e = h.demo.borrow_mut().release(&cs211()).unwrap_err();
    assert!(e.to_string().contains("nothing is released"), "{e}");
    // New exam on demo data says how to open real exams, never a form that grades nothing
    h.shell.navigate(Page::Exams);
    one(&h.ui, "New exam").invoke_accessible_default_action();
    assert!(!h.ui.global::<ExamsVM>().get_new_open());
    let msg = h.ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("demo data") && msg.contains("--workspace"),
        "{msg}"
    );
}
