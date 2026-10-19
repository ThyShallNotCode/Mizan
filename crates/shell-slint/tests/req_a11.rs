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
use shell_slint::screenshot;
use shell_slint::ui::{
    App, AppWindow, ExamsVM, MarkState, OutcomesVM, Page, ResultsVM, ReviewVM, ScanVM, SetupVM,
    Theme,
};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use support::a11::{
    by_label, chord, click, click_at, key, one, press_at, release_at, selected_queue_row, special,
    tab, type_text, with_prefix,
};

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
fn req_a11_one_mouse_click_activates_each_tab_and_button() {
    // Mouse users, not just the accessibility actions above: a keyboard FocusScope declared
    // over a TouchArea took the first press to gain focus, so every tab and button needed two
    // clicks. One press-and-release must switch the page.
    let h = harness();
    let ui = &h.ui;
    let tabs = [
        ("Setup", Page::Setup),
        ("Scan", Page::Scan),
        ("Review", Page::Review),
        ("Results", Page::Results),
        ("Outcomes", Page::Outcomes),
        ("Settings", Page::Settings),
        ("Exams", Page::Exams),
    ];
    for (label, page) in tabs {
        let e = with_prefix(ui, label)
            .into_iter()
            .map(|(_, e)| e)
            .find(|e| e.accessible_role() == Some(AccessibleRole::Tab))
            .unwrap_or_else(|| panic!("a tab labelled {label:?}"));
        click(ui, &e);
        assert_eq!(
            ui.global::<App>().get_page(),
            page,
            "one click on {label:?}"
        );
    }
    // a Button (the exam drawer's), not only the rail
    click(ui, &one(ui, "Open results"));
    assert_eq!(ui.global::<App>().get_page(), Page::Results);
}

/// The exam rows on screen: (row index, top, height, right edge).
fn exam_rows(ui: &AppWindow) -> Vec<(usize, f32, f32, f32)> {
    let rows = ui.global::<ExamsVM>().get_rows();
    (0..rows.row_count())
        .filter_map(|i| {
            let e = by_label(ui, &rows.row_data(i)?.label).into_iter().next()?;
            let (p, s) = (e.absolute_position(), e.size());
            Some((i, p.y, s.height, p.x + s.width))
        })
        .collect()
}

/// The minimum window, where the exam drawer lies over the table.
fn minimum_window(h: &Harness) {
    h.ui.window()
        .set_size(slint::LogicalSize::new(1280.0, 800.0));
    slint::platform::update_timers_and_animations();
}

#[test]
fn req_a11_a_click_on_the_narrow_drawer_stays_in_the_drawer() {
    // Under 1420 px the drawer covers the right of the table, and a press on its background
    // went through to the row underneath and changed the exam.
    let h = harness();
    let ui = &h.ui;
    minimum_window(&h);
    let ex = ui.global::<ExamsVM>();
    assert!(
        ex.get_drawer_open(),
        "the drawer of the current exam is open"
    );
    let selected = ex.get_selected();
    let (_, top, height, right) = exam_rows(ui)
        .into_iter()
        .find(|r| r.0 as i32 != selected)
        .expect("a row other than the selected one");
    // the drawer's right margin, beside no control, over the right end of that row
    let x = right - 16.0;
    assert!(
        x > one(ui, "Open results").absolute_position().x,
        "the spot is on the drawer"
    );
    click_at(ui, x, top + height / 2.0);
    assert_eq!(
        ex.get_selected(),
        selected,
        "the exam behind stays unselected"
    );
    assert!(ex.get_drawer_open());
}

#[test]
fn req_a11_a_click_between_the_new_exam_buttons_reaches_nothing_behind() {
    // The New-exam form floats over the table: a press between its buttons took focus from
    // the field being typed in, and the next one selected the exam behind the form.
    let h = harness();
    let ui = &h.ui;
    minimum_window(&h);
    let ex = ui.global::<ExamsVM>();
    ex.set_new_open(true);
    slint::platform::update_timers_and_animations();
    let left = one(ui, "Import from Remark export…");
    let right = one(ui, "Cancel");
    let (lp, ls, rp) = (
        left.absolute_position(),
        left.size(),
        right.absolute_position(),
    );
    let (gx, gy) = ((lp.x + ls.width + rp.x) / 2.0, lp.y + ls.height / 2.0);
    let rows = exam_rows(ui);
    let behind = rows
        .iter()
        .find(|r| gy >= r.1 && gy < r.1 + r.2)
        .map(|r| r.0)
        .expect("an exam row behind the gap");
    // make another exam current, so a press that falls through would show
    let other = rows
        .iter()
        .find(|r| r.0 != behind)
        .expect("a second exam")
        .0;
    ex.invoke_select(ex.get_rows().row_data(other).expect("row").id);
    ex.set_new_open(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(ex.get_selected(), other as i32);
    // the form opens with the focus in its first field
    type_text(ui, "a");
    click_at(ui, gx, gy);
    type_text(ui, "b");
    click_at(ui, gx, gy);
    assert_eq!(
        ex.get_selected(),
        other as i32,
        "the exam behind the form stays unselected"
    );
    assert!(ex.get_new_open(), "the form stays open");
    assert_eq!(ex.get_new_scans(), "ab", "the field kept the focus");
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
fn req_a11_a_clicked_weight_cell_keeps_what_is_typed() {
    // Focusing a cell selected it by rebuilding every matrix row, which destroyed the text
    // input that had just taken the focus: what was typed next went nowhere. Then the click
    // left the caret where it landed, so typing 0.5 into a cell holding 1 saved 0.51.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    assert_eq!(ui.global::<App>().get_page(), Page::Setup);
    assert_eq!(
        one(ui, "Q02 CLO1 weight").accessible_value().as_deref(),
        Some("1")
    );
    click(ui, &one(ui, "Q02 CLO1 weight"));
    type_text(ui, "0.5");
    special(ui, Key::Return);
    let weight =
        |h: &Harness| h.demo.borrow().exam_setup(&cs211()).unwrap().mapping[1].cells[0].weight;
    assert_eq!(weight(&h), Some(0.5));
    assert_eq!(ui.global::<App>().get_message(), "Weight Q02 × CLO1: 0.5");
    assert_eq!(ui.global::<SetupVM>().get_sel_title(), "Q02 × CLO1");
    // the save refreshed the matrix, and the cell still has the focus
    chord(ui, Key::Control, "a");
    type_text(ui, "1");
    special(ui, Key::Return);
    assert_eq!(weight(&h), Some(1.0));
}

fn setup_cell(h: &Harness, row: usize, clo: usize) -> shell_slint::backend::MappingCell {
    h.demo.borrow().exam_setup(&cs211()).unwrap().mapping[row].cells[clo].clone()
}

#[test]
fn req_a11_the_cell_editor_saves_to_the_cell_it_edits() {
    // The editor beside the matrix committed, when it lost the focus, to the selected cell; a
    // click on another matrix cell had selected that one first, so the value landed there.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    click(ui, &one(ui, "Q02 CLO1 weight"));
    let q03 = setup_cell(&h, 2, 1);
    assert_ne!(q03.weight, Some(0.25));
    click(ui, &one(ui, "Weight of Q02 × CLO1"));
    type_text(ui, "0.25");
    click(ui, &one(ui, "Q03 CLO2 weight"));
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.25));
    assert_eq!(
        setup_cell(&h, 2, 1).weight,
        q03.weight,
        "the cell clicked next"
    );
    // the editor shows the cell clicked
    let su = ui.global::<SetupVM>();
    assert_eq!(su.get_sel_title(), "Q03 × CLO2");
    assert_eq!(
        one(ui, "Weight of Q03 × CLO2").accessible_value(),
        one(ui, "Q03 CLO2 weight").accessible_value()
    );
    // the note editor alike
    click(ui, &one(ui, "Note of Q03 × CLO2"));
    type_text(ui, "big-O");
    click(ui, &one(ui, "Q02 CLO1 weight"));
    assert_eq!(setup_cell(&h, 2, 1).note, "big-O");
    assert_ne!(setup_cell(&h, 1, 0).note, "big-O");
    assert_eq!(su.get_sel_title(), "Q02 × CLO1");
    // a value equal to the next cell's is saved too (it differs from the cell edited)
    click(ui, &one(ui, "Q03 CLO2 weight"));
    click(ui, &one(ui, "Weight of Q03 × CLO2"));
    type_text(ui, "0.25");
    click(ui, &one(ui, "Q02 CLO1 weight"));
    assert_eq!(setup_cell(&h, 2, 1).weight, Some(0.25));
}

#[test]
fn req_a11_the_selection_follows_the_focus_past_an_edit() {
    // Saving a weight moved the selection back to the cell saved after Tab had moved the focus
    // on: the selection mark and the cell editor stayed one cell behind the focus.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    click(ui, &one(ui, "Q02 CLO1 weight"));
    type_text(ui, "0.5");
    special(ui, Key::Tab);
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.5));
    let su = ui.global::<SetupVM>();
    assert_eq!(su.get_sel_title(), "Q02 × CLO2");
    let cells = su.get_rows().row_data(1).expect("row Q02").cells;
    let selected: Vec<bool> = (0..2)
        .map(|c| cells.row_data(c).expect("cell").selected)
        .collect();
    assert_eq!(selected, [false, true]);
    // what is typed now goes to the cell with the focus
    type_text(ui, "0.5");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 1, 1).weight, Some(0.5));
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.5));
}

/// The window loses and regains the focus (an alt-tab), with an event-loop turn after each: the
/// change handlers see the cell lose the focus and take it again.
fn switch_window_away_and_back(ui: &AppWindow) {
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(false));
    slint::platform::update_timers_and_animations();
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    slint::platform::update_timers_and_animations();
}

#[test]
fn req_a11_a_window_switch_keeps_what_was_typed_in_a_cell() {
    // A cell selected all of its text whenever it took the focus, the window becoming active
    // again too: what was typed after an alt-tab replaced what was typed before it. And a click
    // inside a note to add to it selected the whole note.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    click(ui, &one(ui, "Q03 CLO2 weight"));
    click(ui, &one(ui, "Note of Q03 × CLO2"));
    type_text(ui, "abc");
    switch_window_away_and_back(ui);
    type_text(ui, "def");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 2, 1).note, "abcdef");
    // a click at the end of a saved note puts the caret there: typing adds to the note
    click(ui, &one(ui, "Q03 CLO2 weight"));
    let note = one(ui, "Note of Q03 × CLO2");
    let (p, s) = (note.absolute_position(), note.size());
    click_at(ui, p.x + s.width - 10.0, p.y + s.height / 2.0);
    type_text(ui, "!");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 2, 1).note, "abcdef!");
    // a weight cell alike
    click(ui, &one(ui, "Q02 CLO1 weight"));
    type_text(ui, "0.2");
    switch_window_away_and_back(ui);
    type_text(ui, "5");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.25));
}

#[test]
fn req_a11_a_pointer_that_moves_during_a_click_still_replaces_the_weight() {
    // A click selected the cell's value only when the cell took the focus, after the press: a
    // pointer that moved a pixel before the release dragged the caret back and undid the
    // selection, so 0.5 typed into a cell holding 1 saved 0.51.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    let cell = one(ui, "Q02 CLO1 weight");
    assert_eq!(cell.accessible_value().as_deref(), Some("1"));
    let (p, s) = (cell.absolute_position(), cell.size());
    let (x, y) = (p.x + s.width / 2.0, p.y + s.height / 2.0);
    press_at(ui, x, y);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x + 1.0, y),
    });
    release_at(ui, x + 1.0, y);
    type_text(ui, "0.5");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.5));
    assert_eq!(ui.global::<App>().get_message(), "Weight Q02 × CLO1: 0.5");
}

#[test]
fn req_a11_the_cell_editor_shows_a_weight_saved_as_it_took_the_focus() {
    // The matrix cell left behind saves its weight after the editor beside the matrix has the
    // focus; the editor, focused, kept showing the old value, and Enter saved it back.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    click(ui, &one(ui, "Q03 CLO2 weight"));
    click(ui, &one(ui, "Q02 CLO1 weight"));
    type_text(ui, "0.5");
    click(ui, &one(ui, "Weight of Q02 × CLO1"));
    slint::platform::update_timers_and_animations();
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.5));
    assert_eq!(
        one(ui, "Weight of Q02 × CLO1")
            .accessible_value()
            .as_deref(),
        Some("0.5")
    );
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.5));
    // the value shown is selected: typing replaces it
    type_text(ui, "0.25");
    special(ui, Key::Return);
    assert_eq!(setup_cell(&h, 1, 0).weight, Some(0.25));
}

/// The answers of question `row` (0-based) in each key version.
fn key_answers(h: &Harness, row: usize) -> Vec<String> {
    let setup = h.demo.borrow().exam_setup(&cs211()).unwrap();
    setup.keys.iter().map(|k| k.answers[row].clone()).collect()
}

#[test]
fn req_a11_a_weight_typed_into_a_key_cell_never_changes_the_key() {
    // Tab from a row's last weight cell reaches the next row's key cells, and a key cell took
    // any character: a weight typed there set the key by position (1 = A), a published key
    // change.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    let before = key_answers(&h, 2);
    click(ui, &one(ui, "Q02 CLO5 weight"));
    type_text(ui, "0.5");
    special(ui, Key::Tab);
    assert_eq!(setup_cell(&h, 1, 4).weight, Some(0.5));
    type_text(ui, "1");
    special(ui, Key::Tab);
    type_text(ui, "0.5");
    assert_eq!(key_answers(&h, 2), before, "Q03's key is unchanged");
    let msg = ui.global::<App>().get_message().to_string();
    assert!(msg.contains("type a key letter"), "{msg}");
    // a letter still sets the key in the cell with the focus
    type_text(ui, "c");
    assert_eq!(key_answers(&h, 2)[1], "C");
}

#[test]
fn req_a11_shift_and_the_arrows_in_a_key_cell_are_no_key_entries() {
    // A key cell sent Shift, the arrows and Escape to the backend as key entries: an error
    // showed while typing a capital and stayed after Shift+Tab.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Setup"));
    click(ui, &one(ui, "Q02 CLO5 weight"));
    special(ui, Key::Tab);
    type_text(ui, "c");
    assert_eq!(key_answers(&h, 2)[0], "C");
    let su = ui.global::<SetupVM>();
    assert_eq!(su.get_error(), "");
    for k in [Key::Shift, Key::RightArrow, Key::CapsLock, Key::Escape] {
        special(ui, k);
        assert_eq!(su.get_error(), "", "{k:?} is no key entry");
    }
    chord(ui, Key::Shift, "d");
    assert_eq!(key_answers(&h, 2)[0], "D");
    assert_eq!(su.get_error(), "");
    assert_eq!(ui.global::<App>().get_message(), "Key Q03 version A: D");
}

// ------------------------------------------------------------------------------------ review

#[test]
fn req_a11_a_reviewer_note_stays_with_its_booklet() {
    // The note field kept its text when another item was opened (only a typed student number
    // was cleared), and Accept saved it with that item's decision: one student's personal note
    // in another student's record. Another exam showed it too.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Review, 30 open"));
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0412, MRK-001");
    let rv = ui.global::<ReviewVM>();
    click(ui, &one(ui, "Reviewer note (personal data, PDPL)"));
    type_text(ui, "seen ill");
    assert_eq!(rv.get_note(), "seen ill");
    click(ui, &one(ui, "Q17 Form B, booklet 0377, MRK-001"));
    assert_eq!(selected_queue_row(ui), "Q17 Form B, booklet 0377, MRK-001");
    assert_eq!(rv.get_note(), "");
    click(ui, &one(ui, "Accept suggestion"));
    let (action, serial, _) = last_action(&h);
    assert_eq!(serial, "0377");
    assert!(
        matches!(&action, ReviewAction::Resolve { note, .. } if note.is_empty()),
        "{action:?}"
    );
    // a note being typed survives a refresh of the item it is typed for
    click(ui, &one(ui, "Reviewer note (personal data, PDPL)"));
    type_text(ui, "x");
    h.shell.navigate(Page::Review);
    assert_eq!(rv.get_note(), "x");
    // another exam never shows it
    h.shell.select_exam(&ExamId(MATH101.into()));
    assert_eq!(rv.get_note(), "");
}

/// The number of decisions recorded on the demo exam.
fn decisions(h: &Harness) -> usize {
    h.demo.borrow().review_history(&cs211()).len()
}

#[test]
fn req_a11_a_click_on_a_view_control_leaves_enter_to_the_decision() {
    // The view chips, the group headers and the contact tiles kept the keyboard after a
    // click: Enter then zoomed, toggled or collapsed instead of committing the decision.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Review, 30 open"));
    let rv = ui.global::<ReviewVM>();
    let first = |prefix: &str, role: AccessibleRole| {
        with_prefix(ui, prefix)
            .into_iter()
            .map(|(_, e)| e)
            .find(|e| e.accessible_role() == Some(role))
            .unwrap_or_else(|| panic!("a control labelled {prefix:?}…"))
    };
    let controls = [
        "Open booklet ",
        "MRK-001 ",
        "Zoom ",
        "Overlay, Space",
        "Registered, F2",
    ];
    for prefix in controls {
        let n = decisions(&h);
        let control = first(prefix, AccessibleRole::Button);
        click(ui, &control);
        if prefix == "MRK-001 " {
            // collapsed, then expanded again
            click(ui, &first(prefix, AccessibleRole::Button));
        }
        let (zoom, overlay) = (rv.get_zoom(), rv.get_overlay());
        key(ui, "d");
        assert_eq!(h.shell.pending(), Some(Decision::Option(3)), "{prefix}");
        special(ui, Key::Return);
        assert_eq!(
            decisions(&h),
            n + 1,
            "Enter decides after a click on {prefix}…"
        );
        assert_eq!(
            (rv.get_zoom(), rv.get_overlay()),
            (zoom, overlay),
            "{prefix}"
        );
    }
}

#[test]
fn req_a11_after_a_click_on_the_top_bar_or_a_row_the_review_keys_work() {
    // The top bar sits outside the review canvas: after a click on Help or the theme switch,
    // the review keys went to the window and did nothing. A click on a queue row moved no
    // focus, so a key typed after a note went into the note.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Review, 30 open"));
    click(ui, &one(ui, "Help for MRK-001 (F1)"));
    assert!(ui.global::<ReviewVM>().get_help_open());
    key(ui, "d");
    assert_eq!(h.shell.pending(), Some(Decision::Option(3)));
    special(ui, Key::F1);
    assert!(
        !ui.global::<ReviewVM>().get_help_open(),
        "F1 closes the help"
    );
    click(ui, &one(ui, "Dark theme"));
    key(ui, "b");
    assert_eq!(h.shell.pending(), Some(Decision::Option(1)));
    click(ui, &one(ui, "Light theme"));
    // the note, then another row: the next key decides, never types
    click(ui, &one(ui, "Reviewer note (personal data, PDPL)"));
    type_text(ui, "x");
    click(ui, &one(ui, "Q17 Form B, booklet 0377, MRK-001"));
    key(ui, "d");
    assert_eq!(h.shell.pending(), Some(Decision::Option(3)));
    assert_eq!(ui.global::<ReviewVM>().get_note(), "");
}

// ------------------------------------------------------------------------------------ exams

#[test]
fn req_a11_another_exam_opens_without_the_last_exams_drafts() {
    // The Blackboard column ID, typed paths, open bars and the status line stayed when another
    // exam was chosen: Write Blackboard CSV then wrote exam B's grades into exam A's column.
    let h = harness();
    let ui = &h.ui;
    let rs = ui.global::<ResultsVM>();
    let su = ui.global::<SetupVM>();
    let sc = ui.global::<ScanVM>();
    click(ui, &tab(ui, "Results"));
    click(ui, &one(ui, "Export…"));
    assert!(rs.get_export_open());
    // the column of a written upload (the demo's field is disabled: it writes no upload)
    rs.set_blackboard_column("835932".into());
    su.set_key_open(true);
    su.set_key_path("/exams/a/key-A.csv".into());
    su.set_ks_bar_open(true);
    su.set_ks_path("/exams/a/key-sheets".into());
    su.set_clo_path("/exams/a/clos.csv".into());
    sc.set_add_open(true);
    sc.set_add_path("/scans/a".into());
    // exam A's last action on the status line
    ui.global::<App>()
        .set_message("Results released (key version 1) and exported".into());
    // the same exam chosen again keeps them
    click(ui, &tab(ui, "Exams"));
    let ex = ui.global::<ExamsVM>();
    let rows = ex.get_rows();
    let row = |i: usize| rows.row_data(i).expect("row");
    let current = row(ex.get_selected() as usize);
    let (x, y) = left_of(&one(ui, &current.label));
    click_at(ui, x, y);
    assert_eq!(rs.get_blackboard_column(), "835932");
    assert!(sc.get_add_open());
    // another exam
    let other = (0..rows.row_count())
        .map(row)
        .find(|r| r.id != current.id)
        .expect("a second exam");
    let (x, y) = left_of(&one(ui, &other.label));
    click_at(ui, x, y);
    assert_eq!(row(ex.get_selected() as usize).id, other.id);
    assert!(!rs.get_export_open());
    assert_eq!(rs.get_blackboard_column(), "");
    assert!(!rs.get_release_open());
    assert!(!su.get_key_open());
    assert_eq!(su.get_key_path(), "");
    assert!(!su.get_ks_bar_open());
    assert_eq!(su.get_ks_path(), "");
    assert_eq!(su.get_clo_path(), "");
    assert!(!sc.get_add_open());
    assert_eq!(sc.get_add_path(), "");
    assert_eq!(ui.global::<App>().get_message(), "");
}

#[test]
fn req_a11_queue_rows_outlive_a_refresh() {
    // A refresh replaced the queue's model, so Slint rebuilt every row: hover, a press in
    // progress and the elements themselves were gone after each click.
    let h = harness();
    let ui = &h.ui;
    click(ui, &tab(ui, "Review, 30 open"));
    let queue = ui.global::<ReviewVM>().get_queue();
    let rows: Vec<String> = (0..queue.row_count())
        .filter_map(|i| queue.row_data(i))
        .filter(|e| !e.header)
        .map(|e| e.a11y.to_string())
        .collect();
    assert!(rows.len() >= 3, "a queue of several items");
    let other = one(ui, &rows[2]);
    // the same data again (looking the row up brings the list up to date)
    h.shell.navigate(Page::Review);
    one(ui, &rows[2]);
    assert!(
        other.is_valid(),
        "a refresh with unchanged data keeps the row"
    );
    // a click selects its row: only the two rows whose selection changed are updated
    let row = one(ui, &rows[1]);
    click(ui, &row);
    assert!(row.is_valid(), "the clicked row stays");
    assert_eq!(row.accessible_item_selected(), Some(true));
    assert!(other.is_valid());
    assert_eq!(selected_queue_row(ui), rows[1]);
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

/// The demo backend, counting the calls whose first answer is costly on a real exam: the
/// Scan screen's batch (it decodes the page thumbnails), the outcomes and the review items
/// (their crops).
#[derive(Default)]
struct Counting {
    demo: DemoBackend,
    batches: std::cell::Cell<u32>,
    outcomes: std::cell::Cell<u32>,
    items: std::cell::Cell<u32>,
}

impl Backend for Counting {
    fn dashboard(&self) -> shell_slint::backend::Dashboard {
        self.demo.dashboard()
    }
    fn default_exam(&self) -> Option<ExamId> {
        self.demo.default_exam()
    }
    fn exam_setup(&self, exam: &ExamId) -> Option<shell_slint::backend::ExamSetup> {
        self.demo.exam_setup(exam)
    }
    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_key(exam, version, question, answer)
    }
    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_mapping_weight(exam, question, clo, weight)
    }
    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_mapping_note(exam, question, clo, note)
    }
    fn batch_progress(&self, exam: &ExamId) -> Option<shell_slint::backend::BatchProgress> {
        self.batches.set(self.batches.get() + 1);
        self.demo.batch_progress(exam)
    }
    fn review_queue(&self, exam: &ExamId, lane: Lane) -> shell_slint::backend::ReviewQueue {
        self.demo.review_queue(exam, lane)
    }
    fn review_item(
        &self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
    ) -> Option<shell_slint::backend::ReviewItem> {
        self.items.set(self.items.get() + 1);
        self.demo.review_item(exam, item)
    }
    fn apply(
        &mut self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
        action: ReviewAction,
    ) -> Result<shell_slint::backend::ApplyOutcome, shell_slint::backend::BackendError> {
        self.demo.apply(exam, item, action)
    }
    fn undo(
        &mut self,
        exam: &ExamId,
    ) -> Result<Option<shell_slint::backend::ItemId>, shell_slint::backend::BackendError> {
        self.demo.undo(exam)
    }
    fn review_history(&self, exam: &ExamId) -> Vec<shell_slint::backend::DecisionRecord> {
        self.demo.review_history(exam)
    }
    fn results(&self, exam: &ExamId) -> Option<shell_slint::backend::Results> {
        self.demo.results(exam)
    }
    fn outcomes(&self, exam: &ExamId) -> Option<shell_slint::backend::Outcomes> {
        self.outcomes.set(self.outcomes.get() + 1);
        self.demo.outcomes(exam)
    }
}

#[test]
fn req_a11_opening_an_exam_does_the_work_of_the_screen_on_show_only() {
    // The first look at an exam froze the window for about a second: every screen was
    // refreshed, shown or not, and Scan's page thumbnails and the outcomes are costly at first.
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let counted = Rc::new(RefCell::new(Counting::default()));
    let dynamic: Rc<RefCell<dyn Backend>> = counted.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    let calls = || {
        let c = counted.borrow();
        (c.batches.get(), c.outcomes.get(), c.items.get())
    };
    assert_eq!(calls(), (0, 0, 0), "the Exams screen needs none of them");
    let ex = ui.global::<ExamsVM>();
    let other = (0..ex.get_rows().row_count())
        .find(|&i| i as i32 != ex.get_selected())
        .expect("a second exam");
    ex.invoke_select(ex.get_rows().row_data(other).expect("row").id);
    assert_eq!(calls(), (0, 0, 0), "nor does opening another exam from it");
    // each screen reads its part when it is shown
    click(&ui, &tab(&ui, "Scan"));
    assert_eq!(calls(), (1, 0, 0));
    click(&ui, &tab(&ui, "Outcomes"));
    assert_eq!(calls(), (1, 1, 0));
    let review = with_prefix(&ui, "Review")
        .into_iter()
        .map(|(_, e)| e)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Tab))
        .expect("the Review tab");
    click(&ui, &review);
    assert!(calls().2 > 0, "Review opens the first item");
    assert!(shell.current_item().is_some());
}

/// The demo backend with what a test changes behind the shell: key sheets for one exam
/// (confirmed or pending), the CLO approval (none yet) and file, and a background job whose every `pump`
/// reverses the exams table, the Scan screen's tiles and codes (an exam whose batch ends
/// moves to the top) and the Results item rows (their flags drop, as a stability report reorders them).
#[derive(Default)]
struct Hooked {
    demo: DemoBackend,
    key_sheets: Option<(ExamId, bool)>,
    approved: bool,
    clos_replaced: bool,
    reorders: bool,
    reversed: bool,
}

impl Backend for Hooked {
    fn dashboard(&self) -> shell_slint::backend::Dashboard {
        let mut d = self.demo.dashboard();
        for e in &mut d.exams {
            e.mapping_approval = self.approved;
            if self.clos_replaced {
                e.mapped_items = 0;
            }
        }
        if self.reversed {
            d.exams.reverse();
        }
        d
    }
    fn default_exam(&self) -> Option<ExamId> {
        self.demo.default_exam()
    }
    fn exam_setup(&self, exam: &ExamId) -> Option<shell_slint::backend::ExamSetup> {
        self.demo.exam_setup(exam)
    }
    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_key(exam, version, question, answer)
    }
    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_mapping_weight(exam, question, clo, weight)
    }
    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), shell_slint::backend::BackendError> {
        self.demo.set_mapping_note(exam, question, clo, note)
    }
    fn batch_progress(&self, exam: &ExamId) -> Option<shell_slint::backend::BatchProgress> {
        let mut p = self.demo.batch_progress(exam)?;
        if self.reversed {
            p.thumbnails.reverse();
            p.exceptions.reverse();
        }
        Some(p)
    }
    fn review_queue(&self, exam: &ExamId, lane: Lane) -> shell_slint::backend::ReviewQueue {
        self.demo.review_queue(exam, lane)
    }
    fn review_item(
        &self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
    ) -> Option<shell_slint::backend::ReviewItem> {
        self.demo.review_item(exam, item)
    }
    fn apply(
        &mut self,
        exam: &ExamId,
        item: shell_slint::backend::ItemId,
        action: ReviewAction,
    ) -> Result<shell_slint::backend::ApplyOutcome, shell_slint::backend::BackendError> {
        self.demo.apply(exam, item, action)
    }
    fn undo(
        &mut self,
        exam: &ExamId,
    ) -> Result<Option<shell_slint::backend::ItemId>, shell_slint::backend::BackendError> {
        self.demo.undo(exam)
    }
    fn review_history(&self, exam: &ExamId) -> Vec<shell_slint::backend::DecisionRecord> {
        self.demo.review_history(exam)
    }
    fn results(&self, exam: &ExamId) -> Option<shell_slint::backend::Results> {
        let mut r = self.demo.results(exam)?;
        if self.reversed {
            // no flag rank any more: the rows fall back to question order
            for it in &mut r.items {
                it.flags.clear();
            }
        }
        Some(r)
    }
    fn outcomes(&self, exam: &ExamId) -> Option<shell_slint::backend::Outcomes> {
        self.demo.outcomes(exam)
    }
    fn key_sheets(&self, exam: &ExamId) -> Option<shell_slint::backend::KeySheetSummary> {
        let (with, pending) = self.key_sheets.as_ref()?;
        (exam == with).then(|| shell_slint::backend::KeySheetSummary {
            source: "Key sheets graded with the exam".into(),
            preview: false,
            pending: *pending,
            superseded: false,
            versions: Vec::new(),
            unusable: Vec::new(),
            other_versions: Vec::new(),
            consequence: String::new(),
            notes: Vec::new(),
        })
    }
    fn approve_mappings(
        &mut self,
        _: &ExamId,
    ) -> Result<String, shell_slint::backend::BackendError> {
        self.approved = true;
        Ok("Mapping approved".into())
    }
    fn replace_clos(
        &mut self,
        _: &ExamId,
        _: &std::path::Path,
    ) -> Result<String, shell_slint::backend::BackendError> {
        self.clos_replaced = true;
        Ok("CLOs replaced".into())
    }
    fn pump(&mut self) -> bool {
        if self.reorders {
            self.reversed = !self.reversed;
        }
        self.reorders
    }
}

fn hooked(backend: Hooked) -> (AppWindow, Rc<RefCell<Hooked>>, Rc<Shell>) {
    i_slint_backend_testing::init_no_event_loop();
    another_hooked(backend)
}

/// [`hooked`], for a test's second window (the platform is set once per test thread).
fn another_hooked(backend: Hooked) -> (AppWindow, Rc<RefCell<Hooked>>, Rc<Shell>) {
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let b = Rc::new(RefCell::new(backend));
    let dynamic: Rc<RefCell<dyn Backend>> = b.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    (ui, b, shell)
}

#[test]
fn req_a11_screenshots_show_the_key_summary_their_shot_names() {
    // Staging a shot read Setup's "has key sheets" flag, which only a Setup refresh sets:
    // with Setup no longer refreshed off show, the key-sheet shot lost its summary. And the
    // summary's first-show opening overrode a shot that wanted the matrix.
    let default = DemoBackend::new().default_exam().expect("an exam");
    i_slint_backend_testing::init_no_event_loop();
    for (exam, pending) in [
        (ExamId(MATH101.into()), false),
        (ExamId(MATH101.into()), true),
        (default.clone(), true),
    ] {
        assert_ne!(ExamId(MATH101.into()), default);
        let (ui, b, shell) = another_hooked(Hooked {
            key_sheets: Some((exam.clone(), pending)),
            ..Hooked::default()
        });
        let shots = screenshot::real_shots(&*b.borrow());
        assert!(shots.iter().any(|s| s.keysheet), "a key-sheet shot");
        // both themes: the same shots twice
        for _ in 0..2 {
            for shot in &shots {
                screenshot::stage(&shell, &*b, shot);
                if shot.page == Page::Setup {
                    assert_eq!(
                        ui.global::<SetupVM>().get_ks_open(),
                        shot.keysheet,
                        "{} with the key sheets of {exam} (pending: {pending})",
                        shot.name
                    );
                }
            }
        }
    }
}

#[test]
fn req_a11_approving_the_clo_mapping_shows_in_the_exams_drawer() {
    // The drawer is filled with the dashboard, which approving the mapping in Setup did not
    // refresh: the drawer still said "mapped" when the instructor went back to it.
    let (ui, _, shell) = hooked(Hooked::default());
    let drawer = || ui.global::<ExamsVM>().get_drawer();
    assert!(ui.global::<ExamsVM>().get_drawer_open());
    assert_eq!(drawer().mapped_word, "mapped");
    shell.navigate(Page::Setup);
    ui.global::<SetupVM>().invoke_approve_mappings();
    assert_eq!(drawer().mapped_word, "approved");
}

#[test]
fn req_a11_replacing_the_clos_shows_in_the_exams_drawer() {
    // Replacing the CLOs in Setup did not refresh the dashboard either.
    let (ui, _, shell) = hooked(Hooked::default());
    let drawer = || ui.global::<ExamsVM>().get_drawer();
    assert!(drawer().mapped > 0);
    shell.navigate(Page::Setup);
    ui.global::<SetupVM>().invoke_replace_clos("clo.csv".into());
    assert_eq!(drawer().mapped, 0);
}

#[test]
fn req_a11_showing_the_exams_screen_reads_the_dashboard() {
    // Showing the Exams screen refreshed nothing: a change the shell did not make itself (a
    // finished job, another program) stayed out of the table and the drawer.
    let (ui, b, shell) = hooked(Hooked {
        clos_replaced: true,
        ..Hooked::default()
    });
    let drawer = || ui.global::<ExamsVM>().get_drawer();
    assert_eq!(drawer().mapped, 0);
    shell.navigate(Page::Setup);
    b.borrow_mut().clos_replaced = false;
    click(&ui, &tab(&ui, "Exams"));
    assert!(drawer().mapped > 0);
}

/// A point inside `e`, near its left end (clear of the drawer that may lie over a row's right).
fn left_of(e: &ElementHandle) -> (f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x + 24.0, p.y + s.height / 2.0)
}

#[test]
fn req_a11_an_exam_row_pressed_during_a_refresh_opens_the_exam_pressed() {
    // Rows were resolved by their index at the release: a refresh between press and release
    // that reorders the table (a finished batch moves its exam to the top) opened the exam
    // that had moved under the pointer.
    let (ui, b, shell) = hooked(Hooked::default());
    let ex = ui.global::<ExamsVM>();
    let rows = ex.get_rows();
    let n = rows.row_count();
    let i = (0..n)
        .find(|&i| i as i32 != ex.get_selected() && n - 1 - i != i)
        .expect("a row that a reversal moves");
    let pressed = rows.row_data(i).expect("row");
    let (x, y) = left_of(&one(&ui, &pressed.label));
    b.borrow_mut().reorders = true;
    press_at(&ui, x, y);
    assert!(shell.pump(), "the job reorders the table");
    assert_ne!(
        ex.get_rows().row_data(i).expect("row").id,
        pressed.id,
        "another exam is in the row pressed"
    );
    release_at(&ui, x, y);
    let selected = ex.get_rows().row_data(ex.get_selected() as usize);
    assert_eq!(selected.expect("a selection").id, pressed.id);
    assert!(ex.get_drawer_open());
}

#[test]
fn req_a11_a_tile_pressed_during_a_refresh_opens_the_sheet_pressed() {
    // The Scan screen refreshes twice a second during a run; a tile read its sheet at the
    // release, so a sheet that moved under the pointer was opened instead.
    let (ui, b, shell) = hooked(Hooked::default());
    click(&ui, &tab(&ui, "Scan"));
    let sc = ui.global::<ScanVM>();
    // a tile of the first row (always shown), with no review code
    let thumbs = sc.get_thumbs();
    let n = thumbs.row_count();
    let t = (0..10)
        .filter_map(|i| thumbs.row_data(i).map(|t| (i, t)))
        .find(|(i, t)| t.code.is_empty() && n - 1 - i != *i)
        .map(|(_, t)| t)
        .expect("a tile without a code");
    let (x, y) = left_of(&one(&ui, &t.label));
    b.borrow_mut().reorders = true;
    press_at(&ui, x, y);
    assert!(shell.pump());
    release_at(&ui, x, y);
    assert_eq!(ui.global::<App>().get_page(), Page::Scan);
    let msg = ui.global::<App>().get_message().to_string();
    assert!(msg.starts_with(&format!("Sheet {}:", t.serial)), "{msg}");
}

#[test]
fn req_a11_a_code_pressed_during_a_refresh_opens_the_code_pressed() {
    // The same for the exceptions by code beside the tiles.
    let (ui, b, shell) = hooked(Hooked::default());
    click(&ui, &tab(&ui, "Scan"));
    let sc = ui.global::<ScanVM>();
    let first = sc.get_codes().row_data(0).expect("a code");
    let row = one(
        &ui,
        &format!("{} {}, {} open", first.code, first.title, first.count),
    );
    let (x, y) = left_of(&row);
    b.borrow_mut().reorders = true;
    press_at(&ui, x, y);
    assert!(shell.pump());
    assert_ne!(
        sc.get_codes().row_data(0).expect("a code").code,
        first.code,
        "another code is in the row pressed"
    );
    release_at(&ui, x, y);
    assert_eq!(ui.global::<App>().get_page(), Page::Review);
    let item = shell.current_item().expect("an item of the code");
    let code = b.borrow().review_item(&cs211(), item).expect("item").code;
    assert_eq!(code, first.code.as_str());
}

/// The labels of the selected rows on show (exam rows, item rows, queue rows).
fn selected_rows(ui: &AppWindow) -> Vec<String> {
    let rows: std::collections::BTreeSet<String> = ElementQuery::from_root(ui)
        .match_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter(|e| e.accessible_item_selected() == Some(true))
        .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
        .collect();
    rows.into_iter().collect()
}

#[test]
fn req_a11_a_results_row_stays_selected_when_the_rows_move() {
    // Item rows were selected by their index: a refresh that reordered them (the flag
    // stability job reporting) highlighted whichever question moved into the row, and the
    // next exam's row at that index showed selected too.
    let (ui, b, shell) = hooked(Hooked::default());
    // tall enough for every item row (a list shows only the rows on screen)
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 4000.0));
    slint::platform::update_timers_and_animations();
    click(&ui, &tab(&ui, "Results"));
    // the question of each selected row ("Q12, p 0.21, …" → "Q12")
    let selected = |ui: &AppWindow| -> Vec<String> {
        selected_rows(ui)
            .iter()
            .filter_map(|l| l.split(',').next())
            .map(str::to_string)
            .collect()
    };
    let row = |i: usize| {
        ui.global::<ResultsVM>()
            .get_items()
            .row_data(i)
            .expect("a row")
    };
    assert!(ui.global::<ResultsVM>().get_items().row_count() > 2);
    let it = row(0);
    click(&ui, &one(&ui, &it.label));
    assert_eq!(selected(&ui), [it.q.to_string()]);
    b.borrow_mut().reorders = true;
    assert!(shell.pump(), "the job reorders the rows");
    assert_ne!(row(0).q, it.q, "another question is in the row clicked");
    assert_eq!(selected(&ui), [it.q.to_string()]);
    // pressed while a refresh moves the rows: the question pressed is selected
    let other = row(1);
    let (x, y) = left_of(&one(&ui, &other.label));
    press_at(&ui, x, y);
    assert!(shell.pump());
    assert_ne!(row(1).q, other.q);
    release_at(&ui, x, y);
    assert_eq!(selected(&ui), [other.q.to_string()]);
    // another exam opens with no item row selected
    b.borrow_mut().reorders = false;
    let ex = ui.global::<ExamsVM>();
    let rows = ex.get_rows();
    let next = (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .find(|r| r.id.as_str() != CS211)
        .expect("a second exam");
    ex.invoke_select(next.id);
    shell.navigate(Page::Results);
    assert!(ui.global::<ResultsVM>().get_items().row_count() > 2);
    assert_eq!(selected(&ui), Vec::<String>::new());
}

/// A spot of a floating bar's background: the middle of the gap between two of its controls.
fn gap_between(ui: &AppWindow, left: &str, right: &str) -> (f32, f32) {
    let (l, r) = (one(ui, left), one(ui, right));
    let (lp, ls, rp) = (l.absolute_position(), l.size(), r.absolute_position());
    assert!(lp.x + ls.width < rp.x, "{left} is left of {right}");
    ((lp.x + ls.width + rp.x) / 2.0, lp.y + ls.height / 2.0)
}

#[test]
fn req_a11_a_click_on_a_bar_gives_it_the_keyboard_back() {
    // The floating bars' Escape scopes no longer take the focus on a press (so a field being
    // typed in keeps it), and nothing else did: after a click elsewhere, a click on the bar
    // left the keyboard outside it, and Escape did nothing.
    let h = harness();
    let ui = &h.ui;
    let ex = ui.global::<ExamsVM>();
    let rs = ui.global::<shell_slint::ui::ResultsVM>();
    let rm = ui.global::<shell_slint::ui::RemarkVM>();
    let sc = ui.global::<ScanVM>();
    type Bar<'a> = (
        &'a str,
        Page,
        &'a str,
        Box<dyn Fn(bool) + 'a>,
        Box<dyn Fn() -> bool + 'a>,
        (&'a str, &'a str),
    );
    let bars: Vec<Bar> = vec![
        (
            "New exam",
            Page::Exams,
            "Exams",
            Box::new(|v| ex.set_new_open(v)),
            Box::new(|| ex.get_new_open()),
            ("Import from Remark export…", "Cancel"),
        ),
        (
            "Import from Remark export",
            Page::Exams,
            "Exams",
            Box::new(|v| {
                ex.set_new_open(v);
                rm.set_new_panel(v);
            }),
            Box::new(|| rm.get_new_panel()),
            ("Back to the form", "Use this import"),
        ),
        (
            "Exports",
            Page::Results,
            "Results",
            Box::new(|v| rs.set_export_open(v)),
            Box::new(|| rs.get_export_open()),
            ("Write Blackboard CSV", "Close"),
        ),
        (
            "Release",
            Page::Results,
            "Results",
            Box::new(|v| rs.set_release_open(v)),
            Box::new(|| rs.get_release_open()),
            ("Release", "Cancel"),
        ),
        (
            "Compare with Remark",
            Page::Results,
            "Results",
            Box::new(|v| rm.set_cmp_bar(v)),
            Box::new(|| rm.get_cmp_bar()),
            ("Compare", "Cancel"),
        ),
        (
            "Add scans",
            Page::Scan,
            "Scan",
            Box::new(|v| {
                // the demo adds no scans; the bar's field takes the keyboard when it can
                sc.set_can_add(true);
                sc.set_add_open(v);
            }),
            Box::new(|| sc.get_add_open()),
            ("Add and regrade", "Cancel"),
        ),
    ];
    for (name, page, tab_label, open, is_open, (left, right)) in bars {
        h.shell.navigate(page);
        open(true);
        slint::platform::update_timers_and_animations();
        assert!(is_open(), "{name} opens");
        let (x, y) = gap_between(ui, left, right);
        // the keyboard goes elsewhere (the screen's own tab), then the bar is clicked
        click(ui, &tab(ui, tab_label));
        assert!(is_open(), "{name} stays open");
        click_at(ui, x, y);
        special(ui, Key::Escape);
        assert!(!is_open(), "Escape closes {name} after a click on it");
        open(false);
    }
}

#[test]
fn req_a11_a_click_on_a_bar_leaves_the_keyboard_in_its_field() {
    // A click on the bar's background gives the keyboard to its first control only when no
    // control inside has it: the field being typed in keeps it.
    let h = harness();
    let ui = &h.ui;
    let ex = ui.global::<ExamsVM>();
    ex.set_new_open(true);
    slint::platform::update_timers_and_animations();
    click(ui, &one(ui, "Exam title"));
    type_text(ui, "Mid");
    let (x, y) = gap_between(ui, "Import from Remark export…", "Cancel");
    click_at(ui, x, y);
    type_text(ui, "term");
    assert_eq!(ex.get_new_title(), "Midterm");
    assert_eq!(ex.get_new_scans(), "");
    special(ui, Key::Escape);
    assert!(!ex.get_new_open());
}
