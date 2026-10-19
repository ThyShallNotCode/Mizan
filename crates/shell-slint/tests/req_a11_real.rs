//! A11 against the real core (IMPLEMENTATION.md §1, ADR-001 S7): the synthetic IAU exam
//! (`tests/support/synthetic_exam.rs`, no real data) is graded through omr-app, opened through
//! [`AppBackend`] and reviewed headless through accessible labels: accept a suggestion, choose
//! an answer, mark a blank, decide an identity and a form, undo, while the results view-model
//! follows the rescoring. A second test grades in the background and drives the live Scan
//! screen through `Backend::pump`.
//!
//! Each test runs on its own thread with its own Slint testing platform.

mod support;

use omr_app::ExamProject;
use omr_app::model::ResponseStatus;
use omr_model::Response;
use shell_slint::app_backend::{AppBackend, GradeRequest, JobEvent, Spawn, run_grading};
use shell_slint::backend::{Backend, Decision, ExamId, ItemId, Lane, ReviewAction, ReviewKind};
use shell_slint::bind::Shell;
use shell_slint::ui::{
    App, AppWindow, ExamsVM, OutcomesVM, Page, RemarkVM, ResultsVM, ReviewVM, ScanVM, SetupVM,
    Theme,
};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::a11::{all_texts, by_label, click, key, one, selected_queue_row, tab, with_prefix};
use support::synthetic_exam::{self as synth, MIDTERM};

struct Harness {
    ui: AppWindow,
    app: Rc<RefCell<AppBackend>>,
    shell: Rc<Shell>,
    exam: ExamId,
}

fn harness(app: AppBackend) -> Harness {
    let exam = app.default_exam().expect("an exam");
    harness_for(app, exam)
}

/// A harness for any workspace, even one without an exam (`exam` is then a name that matches
/// none).
fn harness_for(app: AppBackend, exam: ExamId) -> Harness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().expect("window");
    ui.global::<Theme>().set_reduce_motion(true);
    let app = Rc::new(RefCell::new(app));
    let dynamic: Rc<RefCell<dyn Backend>> = app.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().expect("show");
    Harness {
        ui,
        app,
        shell,
        exam,
    }
}

impl Harness {
    fn open_items(&self) -> u32 {
        let q = self.app.borrow().review_queue(&self.exam, Lane::Marks);
        q.lanes.iter().map(|l| l.open).sum()
    }

    fn item(&self, id: ItemId) -> shell_slint::backend::ReviewItem {
        self.app
            .borrow()
            .review_item(&self.exam, id)
            .expect("the item")
    }

    fn mean(&self) -> f64 {
        self.app
            .borrow()
            .results(&self.exam)
            .expect("results")
            .stats
            .mean_pct
    }

    fn last(&self) -> (ReviewAction, bool) {
        let h = self.app.borrow().review_history(&self.exam);
        let r = h.last().expect("a recorded decision");
        (r.action.clone(), r.undone)
    }

    /// The booklet (session index) and question (0-based) of a mark item, from the core.
    fn subject(&self, id: ItemId) -> (usize, usize) {
        self.app
            .borrow()
            .with_project(&self.exam, |p| {
                let it = p.session.review.item(id.0).expect("item");
                (
                    it.booklet.expect("booklet"),
                    usize::from(it.question.expect("question")),
                )
            })
            .expect("project")
    }

    /// The booklet's current response and status, and its percentage in the score run.
    fn booklet(&self, bi: usize, q: usize) -> (Response, ResponseStatus, f64) {
        self.app
            .borrow()
            .with_project(&self.exam, |p| {
                let qs = &p.session.booklets[bi].questions[q];
                let sc = p.scoring.as_ref().expect("scoring");
                let run = sc.run.as_ref().expect("score run");
                let row = sc.row_of[bi].expect("booklet scored");
                let pct = run.students[row]
                    .final_pct
                    .as_ref()
                    .map_or(f64::NAN, |x| *x.numer() as f64 / *x.denom() as f64);
                (qs.current.clone(), qs.status, pct)
            })
            .expect("project")
    }

    /// Chooses `d` by its chip's accessible label and commits it with the Accept button, whose
    /// label names the choice (`word`), or the suggestion when they agree. An override of the
    /// suggestion first needs a reason (§13.4), picked by its label too. Returns the reason.
    fn choose_and_accept(&self, chip: &str, word: &str, d: Decision) -> Option<String> {
        let id = self.shell.current_item().expect("an item");
        let it = self.item(id);
        let suggested = it.suggestion.as_ref().map(|s| s.decision) == Some(d);
        let label = if suggested {
            format!("{chip}, suggested")
        } else {
            chip.to_string()
        };
        one(&self.ui, &label).invoke_accessible_default_action();
        assert_eq!(self.shell.pending(), Some(d));
        let mut reason = None;
        if self.app.borrow().needs_reason(&self.exam, id, d) {
            // the commit waits for a reason; the item's family codes are offered first
            let wait = one(&self.ui, "Choose a reason");
            assert_eq!(wait.accessible_enabled(), Some(false));
            let code = it.reasons.first().expect("reason codes").clone();
            one(&self.ui, &format!("Reason: {}", words(&code))).invoke_accessible_default_action();
            reason = Some(code);
        }
        let accept = if suggested {
            "Accept suggestion".to_string()
        } else {
            format!("Accept {word}")
        };
        one(&self.ui, &accept).invoke_accessible_default_action();
        reason
    }
}

fn letter(o: u8) -> String {
    char::from(b'A' + o).to_string()
}

/// A reason code as the chip shows it: "AMBIGUOUS_MARK" → "Ambiguous mark".
fn words(code: &str) -> String {
    let lower = code.replace('_', " ").to_lowercase();
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[test]
fn req_a11_real_backend_review_flow_rescores_results() {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = synth::build(&MIDTERM, dir.path());
    let app = AppBackend::project(&built.exam_dir, "principal:a11-reviewer").expect("open");
    let h = harness(app);
    let ui = &h.ui;

    // the dashboard row is the graded exam; the review tab names the open items
    let results0 = h.app.borrow().results(&h.exam).expect("results");
    assert_eq!(results0.stats.students, MIDTERM.sheets as u32);
    assert_eq!(results0.stats.roster, MIDTERM.sheets as u32 + 2);
    // item analysis (§12.2): the planted mis-key (form A Q12: nobody chose the printed key, the
    // top group prefers the taught answer) is flagged with high confidence by the
    // flag-stability job, and its row comes first; any flag of low confidence reads
    // "(proposed)" in gold, after the high-confidence ones
    let planted = results0
        .items
        .iter()
        .find(|i| i.label == format!("A-Q{:02}", synth::MISKEY_Q + 1))
        .expect("form A Q12");
    let flag = planted.flags.first().expect("flagged");
    assert!(
        matches!(flag.code.as_deref(), Some("SCO-001" | "SCO-002")),
        "{flag:?}"
    );
    assert!(
        matches!(&flag.stability, shell_slint::backend::FlagStability::High { share: Some(s), .. } if *s >= 0.5),
        "{flag:?}"
    );
    tab(ui, "Results").invoke_accessible_default_action();
    let rows = ui.global::<ResultsVM>().get_items();
    let first = rows.row_data(0).expect("an item row");
    assert!(
        first.label.starts_with("A-Q12,")
            && first.label.contains("SCO-00")
            && first.label.contains("high confidence: fires in"),
        "{}",
        first.label
    );
    let mut seen_low = false;
    for r in rows.iter() {
        if r.stability_kind == 2 {
            seen_low = true;
            assert!(r.flag.ends_with("(proposed)"), "{}", r.flag);
        } else if seen_low && r.stability_kind == 1 {
            panic!("a high-confidence flag after a proposed one: {}", r.label);
        }
    }
    let open0 = h.open_items();
    assert!(open0 >= 10, "the synthetic mix leaves review work: {open0}");
    tab(ui, &format!("Review, {open0} open")).invoke_accessible_default_action();
    assert_eq!(ui.global::<App>().get_page(), Page::Review);
    assert_eq!(h.shell.lane(), Lane::Marks);
    let first = h.shell.current_item().expect("the first marks item");
    assert!(
        selected_queue_row(ui).contains("MRK-001"),
        "{}",
        selected_queue_row(ui)
    );
    // the frame shows the real crop with the engine's overlays and its text summary
    let it = h.item(first);
    assert_eq!(it.kind, ReviewKind::Mark);
    assert_eq!(it.bubbles.len(), usize::from(synth::OPTIONS));
    assert!(
        it.crop.width > 100 && it.crop.rgba.len() == (it.crop.width * it.crop.height * 4) as usize
    );
    assert!(
        it.original.is_none(),
        "no separate original crop is invented"
    );
    let summary = with_prefix(ui, "Item summary: Question ");
    assert_eq!(summary.len(), 1, "one text summary of the item");

    // 1. accept the engine's suggestion
    let sugg = it.suggestion.as_ref().expect("a suggestion").decision;
    let (b1, q1) = h.subject(first);
    let mean_before = h.mean();
    let (_, _, pct_before) = h.booklet(b1, q1);
    one(ui, "Accept suggestion").invoke_accessible_default_action();
    assert_eq!(
        h.last(),
        (
            ReviewAction::Resolve {
                decision: sugg,
                accepted_suggestion: true,
                note: String::new(),
            },
            false
        )
    );
    let (resp, status, pct_after) = h.booklet(b1, q1);
    assert_eq!(status, ResponseStatus::Confirmed);
    assert_eq!(
        resp,
        match sugg {
            Decision::Option(o) => Response::Single(o),
            Decision::Blank => Response::Blank,
            other => panic!("unexpected suggestion {other:?}"),
        }
    );
    // the run was rescored: the class mean moved by exactly this booklet's change
    let n = f64::from(h.app.borrow().results(&h.exam).unwrap().stats.students);
    assert!(((h.mean() - mean_before) * n - (pct_after - pct_before)).abs() < 1e-9);
    assert_eq!(h.open_items(), open0 - 1);
    assert_ne!(h.shell.current_item(), Some(first), "the queue advanced");

    // 2. choose an answer: the keyed option of this booklet's form, by its chip label
    let second = h.shell.current_item().expect("a second item");
    let it2 = h.item(second);
    assert_eq!(it2.kind, ReviewKind::Mark);
    let form = usize::from(it2.form.as_bytes()[0] - b'A');
    let keyed = synth::key(&MIDTERM, form, it2.question as u16 - 1);
    let (b2, q2) = h.subject(second);
    let (_, status, pct_before) = h.booklet(b2, q2);
    assert_eq!(
        status,
        ResponseStatus::Pending,
        "a pending question is not yet scored"
    );
    let mean_before = h.mean();
    let reason = h.choose_and_accept(&letter(keyed), &letter(keyed), Decision::Option(keyed));
    assert!(matches!(
        h.last().0,
        ReviewAction::Resolve { decision: Decision::Option(o), .. } if o == keyed
    ));
    // an override carries its reason into the audited record
    let recorded = h
        .app
        .borrow()
        .review_history(&h.exam)
        .last()
        .unwrap()
        .reason
        .clone();
    assert_eq!(recorded, reason);
    let (resp, _, pct_after) = h.booklet(b2, q2);
    assert_eq!(resp, Response::Single(keyed));
    // one more correct answer of 30 (1 point each): +100/30 percentage points
    assert!(
        (pct_after - pct_before - 100.0 / f64::from(synth::QUESTIONS)).abs() < 1e-9,
        "{pct_before} → {pct_after}"
    );
    assert!(((h.mean() - mean_before) * n - (pct_after - pct_before)).abs() < 1e-9);

    // 3. mark a blank
    let third = h.shell.current_item().expect("a third item");
    let (b3, q3) = h.subject(third);
    let (resp3_before, _, _) = h.booklet(b3, q3);
    let mean_before_blank = h.mean();
    h.choose_and_accept("Blank", "Blank", Decision::Blank);
    assert!(matches!(
        h.last().0,
        ReviewAction::Resolve {
            decision: Decision::Blank,
            ..
        }
    ));
    assert_eq!(h.booklet(b3, q3).0, Response::Blank);
    assert_eq!(h.open_items(), open0 - 3);

    // 4. undo the blank: a compensating event re-opens the item, which is shown again
    one(ui, "Undo last decision (Ctrl+Z)").invoke_accessible_default_action();
    assert!(h.last().1, "the latest event is marked undone");
    assert_eq!(h.shell.current_item(), Some(third));
    let (resp, status, _) = h.booklet(b3, q3);
    assert_eq!((resp, status), (resp3_before, ResponseStatus::Pending));
    assert!((h.mean() - mean_before_blank).abs() < 1e-9);
    assert_eq!(h.open_items(), open0 - 2);

    // 5. identity: the booklet whose number is not on the roster
    one(
        ui,
        &format!("Identity, {}", {
            let q = h.app.borrow().review_queue(&h.exam, Lane::Identity);
            q.groups.iter().map(|g| g.items.len()).sum::<usize>()
        }),
    )
    .invoke_accessible_default_action();
    assert_eq!(h.shell.lane(), Lane::Identity);
    let ids = h.app.borrow().review_queue(&h.exam, Lane::Identity);
    let stranger = ids
        .groups
        .iter()
        .flat_map(|g| &g.items)
        .find(|i| i.detail.contains("not on roster"))
        .map(|i| i.id)
        .expect("the not-on-roster booklet");
    h.shell.select_item(Some(stranger));
    let ev = h.item(stranger).identity.expect("identity evidence");
    assert!(
        !ev.candidates.is_empty(),
        "nearest roster students are listed"
    );
    assert!(
        ev.candidates
            .iter()
            .all(|c| c.national_id_masked.is_empty())
    );
    // a near-zero roster match is listed, never the one-key default (Enter must not assign a
    // number that is not on the roster to somebody else)
    if ev.candidates[0].posterior < 0.5 {
        assert!(h.item(stranger).suggestion.is_none());
        assert_eq!(
            one(ui, "Choose a candidate").accessible_enabled(),
            Some(false)
        );
    }
    h.choose_and_accept("Not on roster", "not on roster", Decision::NotOnRoster);
    assert!(matches!(
        h.last().0,
        ReviewAction::Resolve {
            decision: Decision::NotOnRoster,
            ..
        }
    ));
    // the duplicate decision has no counterpart in the core: refused with remediation
    let other = h
        .app
        .borrow()
        .review_queue(&h.exam, Lane::Identity)
        .groups
        .iter()
        .flat_map(|g| &g.items)
        .map(|i| i.id)
        .next();
    if let Some(other) = other {
        let err = h
            .app
            .borrow_mut()
            .apply(
                &h.exam,
                other,
                ReviewAction::Resolve {
                    decision: Decision::Duplicate,
                    accepted_suggestion: false,
                    note: String::new(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("choose the student"), "{err}");
    }

    // 6. scoring: the booklet whose version bubble is blank takes the inferred form
    h.shell.set_lane(Lane::Scoring);
    h.shell.navigate(Page::Review);
    let forms = h.app.borrow().review_queue(&h.exam, Lane::Scoring);
    let inferred = forms
        .groups
        .iter()
        .find(|g| g.code == "SCO-005")
        .and_then(|g| g.items.first())
        .map(|i| i.id)
        .expect("the SCO-005 booklet");
    h.shell.select_item(Some(inferred));
    let fi = h.item(inferred);
    assert_eq!(fi.kind, ReviewKind::Form);
    let printed: Vec<&str> = fi.bubbles.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(
        &printed[..2],
        ["A", "B"],
        "version bubbles carry printed letters"
    );
    one(ui, "Accept suggestion").invoke_accessible_default_action();
    let form_of = h.app.borrow().with_project(&h.exam, |p| {
        let it = p.session.review.item(inferred.0).unwrap();
        p.session.booklets[it.booklet.unwrap()].form_source
    });
    assert_eq!(form_of, Some(omr_app::model::FormSource::Reviewed));

    // the results view-model equals a full rebuild from the saved project, and the screen
    // prints it
    let live = h.app.borrow().results(&h.exam).expect("results");
    let rebuilt = ExamProject::open_read_only(&built.project)
        .expect("reopen")
        .results_vm()
        .expect("results");
    assert!((live.stats.mean_pct - rebuilt.mean_pct).abs() < 1e-9);
    assert_eq!(live.stats.students, rebuilt.students);
    assert_eq!(live.histogram.counts, rebuilt.histogram);
    tab(ui, "Results").invoke_accessible_default_action();
    assert_eq!(
        ui.global::<ResultsVM>().get_mean().to_string(),
        format!("{:.1}", live.stats.mean_pct)
    );
    assert_eq!(
        ui.global::<ResultsVM>().get_open_errors().to_string(),
        live.stats.open_errors.to_string()
    );
    // accepted, chosen, identity and form; the blank was undone
    assert_eq!(ui.global::<ReviewVM>().get_resolved(), 4);

    // 7. release (§13.4): refused while items are open; every remaining item settled with the
    // engine's proposal (or the plain answer of its kind), then released through the
    // confirmation bar; the Blackboard upload is written only now
    assert!(!ui.global::<ResultsVM>().get_blackboard_ready());
    for lane in [Lane::Marks, Lane::Identity, Lane::Capture, Lane::Scoring] {
        loop {
            let q = h.app.borrow().review_queue(&h.exam, lane);
            let Some(id) = q.groups.iter().flat_map(|g| &g.items).map(|i| i.id).next() else {
                break;
            };
            let it = h.item(id);
            let decision = match (&it.suggestion, it.kind) {
                (Some(s), _) => s.decision,
                (None, ReviewKind::Mark) => Decision::Blank,
                (None, ReviewKind::Identity) => Decision::NotOnRoster,
                (None, _) => Decision::Acknowledge,
            };
            h.app
                .borrow_mut()
                .apply(
                    &h.exam,
                    id,
                    ReviewAction::Resolve {
                        decision,
                        accepted_suggestion: it.suggestion.is_some(),
                        note: String::new(),
                    },
                )
                .unwrap_or_else(|e| panic!("{e}"));
        }
    }
    h.shell.navigate(Page::Results);
    one(ui, "Release results…").invoke_accessible_default_action();
    assert!(ui.global::<ResultsVM>().get_release_open());
    one(ui, "Release").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(msg.contains("Results released"), "{msg}");
    assert!(built.exam_dir.join("blackboard_grade_center.csv").exists());
    // exports from Results: the Blackboard grade-center upload into an existing column,
    // then every export; only usernames and scores leave, never names
    one(ui, "Export…").invoke_accessible_default_action();
    assert!(ui.global::<ResultsVM>().get_export_open());
    assert!(ui.global::<ResultsVM>().get_blackboard_ready());
    one(ui, "Blackboard column ID (optional)").set_accessible_value("835932");
    one(ui, "Write Blackboard CSV").invoke_accessible_default_action();
    let bb = std::fs::read_to_string(built.exam_dir.join("blackboard_grade_center.csv"))
        .expect("the Blackboard upload");
    let header = bb.lines().next().expect("a header");
    assert!(
        header.contains("\"Username\"") && header.ends_with("|835932\""),
        "{header}"
    );
    assert!(bb.lines().count() > 1, "final results are in the upload");
    for r in synth::roster(&MIDTERM) {
        assert!(!bb.contains(&r.name) && !bb.contains(&r.name_ar));
    }
    assert!(
        ui.global::<App>()
            .get_message()
            .contains("blackboard_grade_center.csv")
    );
    one(ui, "Write all exports").invoke_accessible_default_action();
    assert!(built.exam_dir.join("results.csv").exists());
    assert!(built.exam_dir.join("tp154.xlsx").exists());
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("tp154.xlsx") && msg.contains("Blackboard"),
        "{msg}"
    );
}

/// A spawner that holds the work until the test runs it, so the state while the project is
/// handed over can be checked first (the test plays the binary's part, R4).
type Held = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;

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

/// Runs the held work on a thread and pumps the shell until the backend is idle again.
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

fn grade(def: &omr_app::ExamDefinition, scans: &Path, project: &Path) {
    let opts = omr_app::GradeOptions {
        threads: 2,
        project_dir: Some(project.to_path_buf()),
        snapshot_utc: Some(synth::SNAPSHOT.into()),
        operator: "principal:a11".into(),
        max_pages: None,
    };
    ExamProject::grade(def, &[scans.to_path_buf()], &opts, &|_| {})
        .expect("grading")
        .close()
        .expect("close");
}

fn jpeg(img: &image::GrayImage, path: &Path) {
    let bytes = omr_sim::capture::encode_jpeg(img, 88, 200).expect("jpeg");
    std::fs::write(path, bytes).expect("write scan");
}

#[test]
fn req_a11_real_backend_adds_scans_from_the_scan_screen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 8,
        folder: "append-quiz",
        ..MIDTERM
    };
    // day 1: sheets 0–4; day 2: sheets 5–7 (sheet 7 scanned in portrait) and a blank back
    let all = synth::write_scans(&spec, &dir.path().join("all"));
    let (day1, day2) = (dir.path().join("day1"), dir.path().join("day2"));
    for d in [&day1, &day2] {
        std::fs::create_dir_all(d).expect("folder");
    }
    for k in 0..spec.sheets {
        let name = format!("scan-{k:03}.jpg");
        let to = if k < 5 { &day1 } else { &day2 };
        std::fs::rename(all.join(&name), to.join(&name)).expect("move scan");
    }
    let last = day2.join("scan-007.jpg");
    let page = image::load_from_memory(&std::fs::read(&last).expect("scan"))
        .expect("decode")
        .to_luma8();
    jpeg(&image::imageops::rotate90(&page), &last);
    let (w, hgt) = page.dimensions();
    let blank = image::GrayImage::from_fn(w, hgt, |x, y| {
        image::Luma([243 + ((x * 7 + y * 13) % 5) as u8])
    });
    jpeg(&blank, &day2.join("scan-008-back.jpg"));
    let project = dir.path().join("append-quiz/project.omrproj");
    grade(&synth::definition(&spec), &day1, &project);

    let (spawn, held) = held_spawner();
    let mut app = AppBackend::project(&project, "principal:a11").expect("open");
    app.set_spawner(spawn);
    let h = harness(app);
    let ui = &h.ui;
    // one decision before the batch: it carries over
    h.shell.navigate(Page::Review);
    let decided = h.shell.current_item().expect("an item on day 1");
    let (bk, q) = h.subject(decided);
    let page_of = |h: &Harness, bk: usize| {
        h.app
            .borrow()
            .with_project(&h.exam, |p| {
                let b = &p.session.booklets[bk];
                p.session.pages[b.pages[0]].page_id.clone()
            })
            .expect("project")
    };
    let first_page = page_of(&h, bk);
    one(ui, "Accept suggestion").invoke_accessible_default_action();
    let before = h.app.borrow().review_history(&h.exam).len();
    assert_eq!(before, 1);

    // the Scan screen: Add scans…, the folder, Add and regrade
    tab(ui, "Scan").invoke_accessible_default_action();
    let sc = ui.global::<ScanVM>();
    assert!(!sc.get_pausable(), "a file import has no Pause or Stop");
    assert!(with_prefix(ui, "Pause").is_empty());
    assert!(sc.get_can_add());
    one(ui, "Add scans…").invoke_accessible_default_action();
    assert!(sc.get_add_open());
    one(ui, "Scan files or folders to add").set_accessible_value(day2.display().to_string());
    one(ui, "Add and regrade").invoke_accessible_default_action();
    // handed over: the exam is closed while the worker grades, and every command waits
    assert!(h.app.borrow().grading_in_progress());
    assert!(!sc.get_add_open());
    assert!(sc.get_running());
    assert!(sc.get_batch_label().contains("Batch 2 of 2"));
    let err = h.app.borrow_mut().undo(&h.exam).unwrap_err().to_string();
    assert!(err.contains("being added"), "{err}");
    assert!(h.app.borrow().results(&h.exam).is_none());
    assert_eq!(
        h.app.borrow().dashboard().exams[0].status,
        shell_slint::backend::ExamStatus::Scanning
    );
    // the review screen says why its queue is empty (never "no open items"), and the item
    // open before the batch is gone: nothing of the closed exam is offered for a decision
    h.shell.navigate(Page::Review);
    let rv = ui.global::<ReviewVM>();
    assert!(rv.get_empty_text().contains("being graded"));
    assert!(!rv.get_has_item(), "{}", rv.get_caption());
    assert!(with_prefix(ui, "Accept").is_empty());
    assert!(shown(ui, "Nothing to review"));
    tab(ui, "Scan").invoke_accessible_default_action();
    run_held(&h, &held);

    // reopened with every booklet; the notices of the new pages are in the activity
    assert!(!sc.get_running());
    assert_eq!(sc.get_batch_label().to_string(), "Batch 2 of 2 · complete");
    let batch = h.app.borrow().batch_progress(&h.exam).expect("batch");
    let recon = |label: &str| {
        batch
            .reconciliation
            .iter()
            .find(|r| r.label == label)
            .map(|r| r.value.clone())
            .unwrap_or_else(|| panic!("no {label} line"))
    };
    assert_eq!(recon("Booklets"), "8");
    assert_eq!(recon("Blank backs dropped (CAP-012)"), "1");
    assert_eq!(recon("Read turned upright (REG-002)"), "1");
    let activity = batch.activity.join("\n");
    assert!(activity.contains("5 → 8 booklets"), "{activity}");
    assert!(activity.contains("1 decision kept"), "{activity}");
    assert!(
        batch.activity.iter().any(|l| l.starts_with("REG-002 ")),
        "{activity}"
    );
    assert!(
        batch
            .activity
            .iter()
            .any(|l| l.starts_with("CAP-012 ") && l.contains("blank")),
        "{activity}"
    );
    // the pills count sheets: every booklet once
    assert_eq!(
        batch.sheets.auto + batch.sheets.review + batch.sheets.rejected,
        8
    );
    assert_eq!(
        h.app
            .borrow()
            .results(&h.exam)
            .expect("results")
            .stats
            .students,
        8
    );
    // the day-1 decision is still the reviewer's, and still in the history
    let kept = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            let b = p
                .session
                .booklets
                .iter()
                .find(|b| p.session.pages[b.pages[0]].page_id == first_page)
                .expect("the booklet");
            b.questions[q].status
        })
        .expect("project");
    assert_eq!(kept, ResponseStatus::Confirmed);
    assert!(h.app.borrow().review_history(&h.exam).len() >= before);
    // a decision taken before the batch was carried over: undo says so instead of rewriting it
    let err = h.app.borrow_mut().undo(&h.exam).unwrap_err().to_string();
    assert!(err.contains("can no longer be undone"), "{err}");
    // the review selection starts again on the reloaded queue
    h.shell.navigate(Page::Review);
    if let Some(id) = h.shell.current_item() {
        assert!(h.app.borrow().review_item(&h.exam, id).is_some());
    }
}

/// Settles every open item through the backend: the suggestion where there is one, else the
/// first option, acknowledge or "not on roster"; an override gives the item's first reason.
fn resolve_everything(h: &Harness) {
    for _ in 0..1000 {
        let next = Lane::ALL.iter().find_map(|&l| {
            h.app
                .borrow()
                .review_queue(&h.exam, l)
                .groups
                .iter()
                .flat_map(|g| &g.items)
                .map(|i| i.id)
                .next()
        });
        let Some(id) = next else { return };
        let it = h.item(id);
        let sugg = it.suggestion.as_ref().map(|s| s.decision);
        let d = match (it.kind, sugg) {
            (ReviewKind::Notice, _) => Decision::Acknowledge,
            (_, Some(d)) => d,
            (ReviewKind::Identity, None) => Decision::NotOnRoster,
            _ => Decision::Option(0),
        };
        let reason = if h.app.borrow().needs_reason(&h.exam, id, d) {
            it.reasons.first().cloned()
        } else {
            None
        };
        h.app
            .borrow_mut()
            .apply_with_reason(
                &h.exam,
                id,
                ReviewAction::Resolve {
                    decision: d,
                    accepted_suggestion: sugg == Some(d),
                    note: String::new(),
                },
                reason,
            )
            .unwrap_or_else(|e| panic!("{} {id}: {e}", it.code));
    }
    panic!("the queue never emptied");
}

const PAGES: [Page; 7] = [
    Page::Exams,
    Page::Setup,
    Page::Scan,
    Page::Review,
    Page::Results,
    Page::Outcomes,
    Page::Settings,
];

/// Enter, the arrows, Space, letters, a digit and Escape, wherever the focus is.
fn press_keys(ui: &AppWindow) {
    for k in ["\n", "\u{f700}", "\u{f701}", " ", "a", "1", "n", "\u{1b}"] {
        key(ui, k);
    }
}

/// An empty workspace: every screen and key shows an empty state, nothing panics.
#[test]
fn req_a11_real_backend_empty_workspace_never_panics() {
    let empty = tempfile::tempdir().expect("temp dir");
    let app = AppBackend::workspace(empty.path(), "principal:a11");
    assert!(app.default_exam().is_none());
    let h = harness_for(app, ExamId("none".into()));
    for p in PAGES {
        h.shell.navigate(p);
        press_keys(&h.ui);
    }
    assert!(h.app.borrow().dashboard().exams.is_empty());
    assert!(!h.ui.global::<ScanVM>().get_available());
    assert!(!h.ui.global::<ResultsVM>().get_available());
    assert!(h.shell.current_item().is_none());
}

/// What only the demo shows (its exams' courses, its user, engine and workspace) and what the
/// screens showed before a binding filled them: never on screen in a real session.
fn demo_strings() -> Vec<String> {
    let d = shell_slint::demo::DemoBackend::new().dashboard();
    let mut v = vec![d.user.name, d.user.role, d.engine, d.workspace];
    for e in d.exams {
        v.extend([e.course_code, e.course_title, e.course_title_ar, e.batch_id]);
    }
    // its own values only, not a "—" it shows for a value it lacks
    v.retain(|s| s.chars().filter(|c| c.is_alphanumeric()).count() >= 3);
    v.extend(
        [
            "CS 211 Data Structures",
            "Midterm 1",
            "1448 H, 1st term",
            "Fall 2026",
            "Floor 40",
            "Project saved",
            "§",
            "WCAG",
        ]
        .map(String::from),
    );
    v
}

/// Fails, naming the screen and the text, when anything on screen shows one of `banned`, or a
/// "Saved" that no save made.
fn assert_none_shown(ui: &AppWindow, banned: &[String], screen: &str) {
    for t in all_texts(ui) {
        assert_ne!(t, "Saved", "{screen}: a \"Saved\" pill");
        if let Some(b) = banned.iter().find(|b| t.contains(b.as_str())) {
            panic!("{screen} shows {b:?} in {t:?}");
        }
    }
}

/// True when some text on screen contains `part`.
fn shown(ui: &AppWindow, part: &str) -> bool {
    all_texts(ui).iter().any(|t| t.contains(part))
}

/// G-2: a real session with no exam (an empty exams folder) shows nothing of the demo and no
/// placeholder: no course in the breadcrumbs, no capture-agent buttons, no "Saved", no step
/// done. The exams table says the folder is empty and leads to New exam and the Remark import;
/// every screen of one exam says none is open and leads back to Exams.
#[test]
fn req_a11_real_backend_empty_workspace_shows_no_demo_data() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (spawn, _held) = held_spawner();
    let mut app = AppBackend::workspace(dir.path(), "principal:a11");
    app.set_spawner(spawn);
    let h = harness_for(app, ExamId("none".into()));
    let ui = &h.ui;
    let demo = demo_strings();
    for p in PAGES {
        h.shell.navigate(p);
        let screen = format!("{p:?} with no exam");
        assert_none_shown(ui, &demo, &screen);
        let a = ui.global::<App>();
        assert!(!a.get_has_exam(), "{screen}");
        assert_eq!(a.get_course(), "", "{screen}");
        assert_eq!(a.get_assessment(), "", "{screen}");
        if !matches!(p, Page::Exams | Page::Settings) {
            one(ui, "No exam open");
            one(ui, "Go to Exams");
        }
    }

    // Exams: an empty folder, and what to do about it
    h.shell.navigate(Page::Exams);
    one(ui, "No exams in this folder yet");
    assert!(
        by_label(ui, "Operator").is_empty(),
        "no role is made up for the user"
    );
    click(ui, &one(ui, "New exam…"));
    let ex = ui.global::<ExamsVM>();
    assert!(ex.get_new_open(), "New exam… opens the form");
    assert!(
        by_label(ui, "No exams in this folder yet").is_empty(),
        "the form covers the empty state (its buttons are never shown twice)"
    );
    click(ui, &one(ui, "Cancel"));
    assert!(!ex.get_new_open());
    click(ui, &one(ui, "Import from Remark export…"));
    assert!(
        ex.get_new_open(),
        "the Remark import opens on the New-exam form"
    );
    assert!(ui.global::<RemarkVM>().get_new_panel());
    one(ui, "Remark workbooks");
    ex.set_new_open(false);

    // Setup: no step is done or wanting, no key tag, no key buttons; Go to Exams goes there
    h.shell.navigate(Page::Setup);
    for (i, step) in [
        "Template",
        "Forms and key",
        "CLO mapping",
        "Roster",
        "Policies",
    ]
    .iter()
    .enumerate()
    {
        one(ui, &format!("{} {step}, to do", i + 1));
    }
    assert!(!shown(ui, "Key v"), "no key tag");
    assert!(by_label(ui, "Replace key…").is_empty());
    assert!(!ui.global::<SetupVM>().get_has_roster());
    click(ui, &one(ui, "Go to Exams"));
    assert_eq!(ui.global::<App>().get_page(), Page::Exams);

    // Scan: no capture agent to pause or stop, nothing to add scans to, no review to open
    h.shell.navigate(Page::Scan);
    for control in ["Pause", "Resume", "Stop", "Add scans…", "Open review"] {
        assert!(
            with_prefix(ui, control).is_empty(),
            "Scan with no exam offers {control:?}"
        );
    }
    assert!(!shown(ui, "workers"), "no engine line without a batch");

    // Review: no lanes, progress, undo or help for a code that is not there
    h.shell.navigate(Page::Review);
    assert!(by_label(ui, "Review progress").is_empty());
    assert!(by_label(ui, "Undo last decision (Ctrl+Z)").is_empty());
    assert!(with_prefix(ui, "Help for").is_empty());
    assert!(!shown(ui, "audit head"));
    assert!(!shown(ui, "Two-person rule"));
}

/// G-2: nothing of one exam stays on screen when another, or none, is shown: a graded exam's
/// course, key tag, release block and review crop are gone once the shell shows no exam, and
/// once it shows an exam that is being graded (whose screens say so). A fresh exam, graded from
/// the New-exam form, shows its own data and nothing of the demo.
#[test]
fn req_a11_real_backend_never_shows_a_previous_exam_or_the_demo() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let ws = dir.path().join("exams");
    let mut def = synth::definition(&spec);
    def.term = "Spring 2027".into();
    grade(&def, &scans, &ws.join("bare-quiz/project.omrproj"));
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11");
    app.set_spawner(spawn);
    let h = harness(app);
    let ui = &h.ui;
    let demo = demo_strings();

    // the graded exam on every screen: what of it would be stale elsewhere
    for p in PAGES {
        h.shell.navigate(p);
        assert_none_shown(ui, &demo, &format!("{p:?} of a graded exam"));
    }
    h.shell.navigate(Page::Review);
    let caption = ui.global::<ReviewVM>().get_caption().to_string();
    assert!(caption.contains("booklet"), "{caption}");
    let key_status = ui.global::<SetupVM>().get_key_status().to_string();
    assert!(!key_status.is_empty());
    h.shell.navigate(Page::Results);
    let rs = ui.global::<ResultsVM>();
    assert!(rs.get_release_blocked(), "an inferred key blocks release");
    let reason = rs.get_release_reason().to_string();
    let mut stale = vec![
        spec.course_code.to_string(),
        spec.course_title.to_string(),
        caption,
        key_status,
        reason,
    ];
    stale.extend(demo.iter().cloned());

    // the exam is gone (its folder removed, say): no screen keeps any of it
    h.shell.select_exam(&ExamId("gone".into()));
    for p in PAGES {
        h.shell.navigate(p);
        let screen = format!("{p:?} after the exam is gone");
        assert_eq!(ui.global::<App>().get_course(), "", "{screen}");
        assert!(!ui.global::<App>().get_has_exam(), "{screen}");
        if p != Page::Exams {
            assert_none_shown(ui, &stale, &screen);
        }
    }
    assert!(!ui.global::<SetupVM>().get_has_roster());
    assert_eq!(ui.global::<SetupVM>().get_key_status(), "");
    assert!(!ui.global::<ResultsVM>().get_release_blocked());
    assert!(!ui.global::<ReviewVM>().get_has_item());

    // a new exam from the New-exam form, held while it is graded
    h.shell.navigate(Page::Exams);
    click(ui, &one(ui, "New exam"));
    one(ui, "Scan files or folders").set_accessible_value(scans.to_str().unwrap());
    one(ui, "Exam title").set_accessible_value("Quiz 9");
    one(ui, "Course code").set_accessible_value("MATH 299");
    one(ui, "Number of questions").set_accessible_value("30");
    click(ui, &one(ui, "Grade exam"));
    assert!(h.app.borrow().grading_in_progress());
    for p in PAGES {
        h.shell.navigate(p);
        let screen = format!("{p:?} of an exam being graded");
        assert!(ui.global::<App>().get_has_exam(), "{screen}");
        assert!(
            ui.global::<App>().get_course().contains("MATH 299"),
            "{screen}"
        );
        if p != Page::Exams {
            assert_none_shown(ui, &stale, &screen);
        }
    }
    // the screens with nothing yet say why
    h.shell.navigate(Page::Setup);
    assert!(shown(ui, "being graded"), "Setup says why it is empty");
    h.shell.navigate(Page::Results);
    assert!(shown(ui, "being graded"), "Results says why it is empty");
    h.shell.navigate(Page::Outcomes);
    assert!(shown(ui, "being graded"), "Outcomes says why it is empty");
    assert!(!ui.global::<OutcomesVM>().get_empty_to_setup());
    h.shell.navigate(Page::Review);
    one(ui, "No review queue");
    one(ui, "Nothing to review");
    assert!(shown(ui, "being graded"), "the review canvas says why");

    // graded: its own data, nothing of the demo; no CLOs, and Outcomes says where to load them
    run_held(&h, &held);
    for p in PAGES {
        h.shell.navigate(p);
        assert_none_shown(ui, &demo, &format!("{p:?} of the fresh exam"));
    }
    h.shell.navigate(Page::Outcomes);
    one(ui, "No CLOs for this exam");
    click(ui, &one(ui, "Load a CLO file in Setup"));
    assert_eq!(ui.global::<App>().get_page(), Page::Setup);
}

/// Every screen and the keyboard on a project that breaks naive code: no roster, no CLOs, an
/// inferred key, and then no review item left. Nothing panics, every screen shows an honest
/// state, and data the project lacks is never shown as real.
#[test]
fn req_a11_real_backend_edge_projects_never_panic() {
    let pages = PAGES;
    let keys = press_keys;
    // no roster, no CLOs, an inferred key
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let project = dir.path().join("bare-quiz/project.omrproj");
    grade(&synth::definition(&spec), &scans, &project);
    let h = harness(AppBackend::project(&project, "principal:a11").expect("open"));
    let ui = &h.ui;
    for p in pages {
        h.shell.navigate(p);
        keys(ui);
    }
    assert!(
        h.app.borrow().outcomes(&h.exam).is_none(),
        "no CLOs, no outcomes"
    );
    let r = h.app.borrow().results(&h.exam).expect("results");
    assert_eq!(r.stats.roster, 0, "no roster is never shown as one");
    assert!(
        r.key_label.to_uppercase().contains("INFERRED"),
        "{}",
        r.key_label
    );
    assert!(
        r.blackboard_blocked
            .as_deref()
            .is_some_and(|m| m.contains("inferred"))
    );
    h.shell.navigate(Page::Results);
    let rs = ui.global::<ResultsVM>();
    assert!(rs.get_students_caption().contains("no roster"));
    one(ui, "Export…").invoke_accessible_default_action();
    assert!(!rs.get_blackboard_ready());
    assert_eq!(
        one(ui, "Write Blackboard CSV").accessible_enabled(),
        Some(false)
    );
    let setup = h.app.borrow().exam_setup(&h.exam).expect("setup");
    assert!(setup.clos.is_empty());
    assert_eq!(
        h.app.borrow().dashboard().exams[0].roster_count,
        0,
        "no roster students are invented"
    );

    // an override reason that needs a note: typed, then Accept clicked with the mouse
    h.shell.navigate(Page::Review);
    let marks = Lane::ALL.iter().find_map(|&l| {
        let q = h.app.borrow().review_queue(&h.exam, l);
        q.groups
            .iter()
            .flat_map(|g| &g.items)
            .find(|i| i.kind == ReviewKind::Mark)
            .map(|i| (l, i.id))
    });
    assert!(marks.is_some(), "the bare quiz has a mark item to override");
    if let Some((lane, id)) = marks {
        h.shell.set_lane(lane);
        h.shell.select_item(Some(id));
        let it = h.item(id);
        let sugg = it.suggestion.as_ref().map(|s| s.decision);
        let o = (0..it.bubbles.len() as u8)
            .map(Decision::Option)
            .find(|d| Some(*d) != sugg && h.app.borrow().needs_reason(&h.exam, id, *d))
            .expect("an option that overrides the engine");
        h.shell.choose(o);
        // the family's first codes and Other, and every canonical code one chip away (§14.4)
        assert!(it.reasons.len() > 5, "{:?}", it.reasons);
        assert_eq!(with_prefix(ui, "Reason: ").len(), 5);
        one(ui, &format!("{} more reasons…", it.reasons.len() - 5))
            .invoke_accessible_default_action();
        for code in &it.reasons {
            one(ui, &format!("Reason: {}", words(code)));
        }
        one(ui, "Fewer reasons");
        one(ui, "Reason: Other").invoke_accessible_default_action();
        let wait = one(ui, "Write why in the note");
        assert_eq!(wait.accessible_enabled(), Some(false));
        // the note is in the view-model as soon as it is entered (two-way), so the commit that
        // follows reads it whichever way Accept is pressed
        one(ui, "Reviewer note (personal data, PDPL)")
            .set_accessible_value("intent clear from the margin");
        let accept = format!(
            "Accept {}",
            it.bubbles[usize::from(match o {
                Decision::Option(i) => i,
                _ => 0,
            })]
            .label
        );
        one(ui, &accept).invoke_accessible_default_action();
        let last = h
            .app
            .borrow()
            .review_history(&h.exam)
            .pop()
            .expect("recorded");
        assert_eq!(last.reason.as_deref(), Some("OTHER"));
        assert!(matches!(
            last.action,
            ReviewAction::Resolve { ref note, .. } if note == "intent clear from the margin"
        ));
    }

    // an identity without a roster: the student number as written on the sheet, typed
    let ident = h
        .app
        .borrow()
        .review_queue(&h.exam, Lane::Identity)
        .groups
        .iter()
        .flat_map(|g| &g.items)
        .map(|i| i.id)
        .next();
    assert!(
        ident.is_some(),
        "the bare quiz has an identity item to type a number for"
    );
    if let Some(id) = ident {
        h.shell.set_lane(Lane::Identity);
        h.shell.select_item(Some(id));
        assert!(h.item(id).identity.expect("evidence").candidates.is_empty());
        one(ui, "Student number").set_accessible_value("926 004-1262");
        one(ui, "Use number").invoke_accessible_default_action();
        if let Some((_, first)) = with_prefix(ui, "Reason: ").into_iter().next() {
            // the typed number overrides the engine's reading: a reason first
            first.invoke_accessible_default_action();
        }
        one(ui, "Accept student 9260041262").invoke_accessible_default_action();
        let student = h
            .app
            .borrow()
            .with_project(&h.exam, |p| {
                let it = p.session.review.item(id.0).expect("item");
                let b = &p.session.booklets[it.booklet.expect("booklet")];
                (
                    b.identity.student.clone(),
                    b.identity.confirmed,
                    it.is_open(),
                )
            })
            .expect("project");
        assert_eq!(student, (Some("9260041262".into()), true, false));
    }

    // the same project once no review item is left
    resolve_everything(&h);
    assert_eq!(h.open_items(), 0);
    for p in pages {
        h.shell.navigate(p);
        keys(ui);
    }
    h.shell.navigate(Page::Review);
    assert!(h.shell.current_item().is_none());
    assert!(!ui.global::<ReviewVM>().get_has_item());
    one(ui, "Undo last decision (Ctrl+Z)").invoke_accessible_default_action();
    assert!(
        h.shell.current_item().is_some(),
        "undo re-opens the last item"
    );
}

#[test]
fn req_a11_real_backend_grades_live_through_pump() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 8,
        folder: "live-quiz",
        ..MIDTERM
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let def = synth::definition(&spec);
    let (tx, rx) = std::sync::mpsc::channel::<JobEvent>();
    let req = GradeRequest {
        definition: def.clone(),
        inputs: vec![scans],
        key_sheets: Vec::new(),
        options: omr_app::GradeOptions {
            threads: 2,
            project_dir: Some(dir.path().join("live-quiz/project.omrproj")),
            snapshot_utc: Some(synth::SNAPSHOT.into()),
            operator: "principal:a11".into(),
            max_pages: None,
        },
    };
    // the test plays the binary's part: it owns the worker thread
    let worker = std::thread::spawn(move || run_grading(req, tx));
    let h = harness(AppBackend::grading(&def, rx, None, "principal:a11"));
    let ui = &h.ui;
    h.shell.navigate(Page::Scan);
    assert!(
        ui.global::<ScanVM>().get_available(),
        "the live batch shows at once"
    );
    // nothing reviewable exists before the project does: honest empty states
    assert!(h.app.borrow().results(&h.exam).is_none());
    assert!(h.app.borrow().exam_setup(&h.exam).is_none());
    let t0 = Instant::now();
    while h.app.borrow().grading_in_progress() {
        assert!(
            t0.elapsed() < Duration::from_secs(300),
            "grading did not finish"
        );
        if h.app.borrow_mut().pump() {
            h.shell.refresh_all();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.join().expect("worker");
    h.shell.refresh_all();
    assert_eq!(h.app.borrow().grading_error(), None);
    let sc = ui.global::<ScanVM>();
    assert!(!sc.get_running());
    assert_eq!(sc.get_pages_value(), "8");
    assert!(sc.get_batch_label().contains("complete"));
    // the finished project opened: thumbnails carry booklet serials and real page images
    let batch = h.app.borrow().batch_progress(&h.exam).expect("batch");
    assert_eq!(batch.pages_done, 8);
    assert!(batch.thumbnails.iter().all(|t| t.image.is_some()));
    assert!(batch.thumbnails.iter().all(|t| t.serial.len() == 4));
    let results = h
        .app
        .borrow()
        .results(&h.exam)
        .expect("results after grading");
    assert_eq!(results.stats.students, 8);
    let d = h.app.borrow().dashboard();
    assert_eq!(d.exams.len(), 1);
    assert_eq!(d.exams[0].sheets_scanned, 8);
    assert!(d.stats.throughput_pages_per_sec.is_some());
}

/// CR-04, SEC-12: the real key replaces an inferred one from Setup ("Replace key…", a key file
/// path, "Replace and rescore"), audited and rescored; and a project whose snapshot was edited
/// outside the app is not opened, with the SYS-011 reason and what to do, never "close the
/// other program".
#[test]
fn req_a11_real_backend_replaces_the_key_from_setup_and_names_a_tampered_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let project = dir.path().join("bare-quiz/project.omrproj");
    let def = synth::definition(&spec);
    grade(&def, &scans, &project);
    // the instructor's key file (one column per form)
    let mut csv = format!("question,{}\n", def.forms.join(","));
    for q in 0..def.questions {
        csv.push_str(&(q + 1).to_string());
        for f in 0..def.forms.len() {
            csv.push(',');
            csv.push(char::from(b'A' + synth::key(&spec, f, q)));
        }
        csv.push('\n');
    }
    let key = dir.path().join("key.csv");
    std::fs::write(&key, csv).unwrap();
    {
        let h = harness(AppBackend::project(&project, "principal:a11").expect("open"));
        let ui = &h.ui;
        let before = h.app.borrow().results(&h.exam).expect("results").key_label;
        assert!(before.to_uppercase().contains("INFERRED"), "{before}");
        h.shell.navigate(Page::Setup);
        one(ui, "Replace key…").invoke_accessible_default_action();
        one(ui, "Key file").set_accessible_value(key.to_str().unwrap());
        one(ui, "Replace and rescore").invoke_accessible_default_action();
        let msg = ui.global::<App>().get_message().to_string();
        assert!(
            msg.contains("in force") && msg.contains("rescored"),
            "{msg}"
        );
        let after = h.app.borrow().results(&h.exam).expect("results").key_label;
        assert!(!after.to_uppercase().contains("INFERRED"), "{after}");
    }
    // a snapshot edited outside the app: refused with SYS-011 and the remedy
    let session = project.join("cache").join("session.json");
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
    v["definition"]["title"] = serde_json::json!("Edited outside the app");
    std::fs::write(&session, serde_json::to_vec(&v).unwrap()).unwrap();
    let Err(e) = AppBackend::project(&project, "principal:a11") else {
        panic!("a tampered project was opened");
    };
    assert!(e.contains("SYS-011") && e.contains("Restore"), "{e}");
    assert!(!e.contains("Close the other program"), "{e}");
}

/// CR-04: a new exam from the dashboard of a workspace ("New exam", the scans, the question
/// count; no key file, so the key is inferred): a bad entry is refused with what to type, the
/// run is graded in the background into a new folder of the workspace, and the exam opens.
#[test]
fn req_a11_real_backend_creates_an_exam_from_the_new_exam_form() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let ws = dir.path().join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11");
    app.set_spawner(spawn);
    let h = harness_for(app, ExamId("none".into()));
    let ui = &h.ui;
    assert!(ui.global::<ExamsVM>().get_can_create());
    one(ui, "New exam").invoke_accessible_default_action();
    assert!(ui.global::<ExamsVM>().get_new_open(), "the form opens");
    one(ui, "Scan files or folders").set_accessible_value(scans.to_str().unwrap());
    one(ui, "Exam title").set_accessible_value(spec.title);
    one(ui, "Course code").set_accessible_value(spec.course_code);
    // a question count that is not a number, then a key file that does not exist: refused
    one(ui, "Number of questions").set_accessible_value("thirty");
    one(ui, "Grade exam").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Questions") && msg.contains("e.g. 30"),
        "{msg}"
    );
    one(ui, "Number of questions").set_accessible_value("30");
    one(ui, "Answer key file").set_accessible_value("/no/such/key.csv");
    one(ui, "Grade exam").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(msg.contains("check the path"), "{msg}");
    assert!(
        held.borrow().is_empty(),
        "nothing started for a refused form"
    );
    // no key file: the key is inferred, and the message says so
    one(ui, "Answer key file").set_accessible_value("");
    one(ui, "Grade exam").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Grading 6 scan files") && msg.contains("inferred"),
        "{msg}"
    );
    assert!(!ui.global::<ExamsVM>().get_new_open(), "the form closes");
    assert_eq!(ui.global::<App>().get_page(), Page::Scan);
    assert!(!h.app.borrow().can_create_exam(), "one run at a time");
    // before the first page is read nothing is invented: no share, time or estimate, and the
    // run is not "complete"
    let sc = ui.global::<ScanVM>();
    assert!(sc.get_running());
    let tiles = [
        sc.get_auto_value(),
        sc.get_first_value(),
        sc.get_remaining_value(),
    ];
    assert_eq!(tiles.map(|t| t.to_string()), ["–", "–", "–"]);
    assert_eq!(sc.get_auto_caption(), "questions · AR –");
    assert_eq!(sc.get_remaining_caption(), "reading inputs");
    run_held(&h, &held);
    // the status line no longer says the run is grading
    let msg = ui.global::<App>().get_message().to_string();
    assert!(!msg.starts_with("Grading "), "{msg}");
    assert_eq!(h.app.borrow().grading_error(), None);
    let project = ws.join("cs-213-quiz-1/project.omrproj");
    assert!(
        project.exists(),
        "graded into a new folder of the workspace"
    );
    let d = h.app.borrow().dashboard();
    assert_eq!(d.exams.len(), 1);
    assert_eq!(d.exams[0].sheets_scanned, 6);
    let exam = h.app.borrow().default_exam().expect("the new exam");
    let results = h.app.borrow().results(&exam).expect("results");
    assert!(
        results.key_label.to_uppercase().contains("INFERRED"),
        "{}",
        results.key_label
    );
    assert!(h.app.borrow().can_create_exam(), "the next exam can start");
}

/// C7: the New exam form's *Key sheets* field. The instructor's key sheets named there are read
/// with the exam as key sheets (never graded as students), their originals stored, the run held
/// until the key is confirmed in Setup; the confirmation publishes a key version whose source is
/// the key sheets, naming both scans.
#[test]
fn req_a11_real_backend_new_exam_reads_the_named_key_sheets() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        ..synth::BARE
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let keys = synth::write_key_sheets(&spec, &dir.path().join("keys"), false);
    let ws = dir.path().join("exams");
    std::fs::create_dir_all(&ws).expect("workspace");
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::workspace(&ws, "principal:a11-instructor");
    app.set_spawner(spawn);
    let h = harness_for(app, ExamId("none".into()));
    let ui = &h.ui;
    tall(ui);
    one(ui, "New exam").invoke_accessible_default_action();
    one(ui, "Scan files or folders").set_accessible_value(scans.to_str().unwrap());
    one(ui, "Exam title").set_accessible_value(spec.title);
    one(ui, "Course code").set_accessible_value(spec.course_code);
    one(ui, "Number of questions").set_accessible_value("30");
    one(ui, "Form letters").set_accessible_value("A, B");
    let both = format!(
        "{};{}",
        keys[0].to_str().unwrap(),
        keys[1].to_str().unwrap()
    );
    one(ui, "Key sheets").set_accessible_value(&both);
    one(ui, "Grade exam").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Grading 6 scan files") && msg.contains("read from the key sheets"),
        "{msg}"
    );
    run_held(&h, &held);
    assert_eq!(h.app.borrow().grading_error(), None);
    let exam = h.app.borrow().default_exam().expect("the new exam");
    let state = |h: &Harness| {
        h.app
            .borrow()
            .with_project(&exam, |p| {
                let ks = &p.session.key_sheets;
                (
                    ks.sheets.len(),
                    ks.sheets.iter().all(|k| k.declared && k.usable()),
                    p.session.booklets.len(),
                    p.session.key_unconfirmed(),
                    p.scoring.is_some(),
                )
            })
            .expect("project")
    };
    assert_eq!(
        state(&h),
        (2, true, 6, true, false),
        "two named key sheets, six students, held until confirmed"
    );
    // confirmed in Setup, as the message says
    h.shell.navigate(Page::Setup);
    let su = ui.global::<SetupVM>();
    assert!(su.get_has_keysheets());
    one(ui, "Confirm every version").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Key confirmed") && msg.contains("published from the key sheets"),
        "{msg}"
    );
    let (sheets, declared, booklets, unconfirmed, scored) = state(&h);
    assert_eq!((sheets, declared, booklets), (2, true, 6));
    assert!(!unconfirmed && scored, "scored once the key is confirmed");
    let provenance = h
        .app
        .borrow()
        .with_project(&exam, |p| {
            let ev: Vec<_> = p
                .store()
                .expect("store")
                .events()
                .expect("events")
                .into_iter()
                .filter(|e| {
                    e.event_type == "KeyVersionPublished" && e.payload["source"] == "key_sheet"
                })
                .collect();
            let n = ev
                .last()
                .and_then(|e| e.payload["scans"].as_array().map(Vec::len))
                .unwrap_or(0);
            (ev.len(), n)
        })
        .expect("project");
    assert_eq!(provenance, (1, 2), "the key version names both key sheets");
}

/// The labels of the key-sheet rows of the version shown ("Question 7 version A: …"), from
/// the view-model (a list view instantiates only the rows in view).
fn key_rows(ui: &AppWindow) -> Vec<String> {
    ui.global::<SetupVM>()
        .get_ks_rows()
        .iter()
        .map(|r| r.label.to_string())
        .collect()
}

/// A window tall enough for every key-sheet row to be instantiated, so each row's choices can
/// be driven through their labels.
fn tall(ui: &AppWindow) {
    ui.window().set_size(slint::PhysicalSize::new(1440, 2400));
}

/// §10.1 (CR-11): an exam graded with the instructor's key sheets (named, no key file) is held
/// until the key is confirmed, and the confirmation is taken in Setup through accessible
/// labels. The key sheets are listed on Scan as key sheets, never as students; Results says
/// why there is nothing yet and leads to Setup; the three rows of version A that wait for a
/// decision (two marks, no mark, a faint mark) are highlighted and block the confirmation
/// until each is decided; version A is confirmed alone (still held), then version B lifts the
/// hold. The confirmation is recorded with the operator and the scans, and the scores equal
/// grading the same students with the same key as a file.
#[test]
fn req_a11_real_backend_confirms_the_key_from_key_sheets() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 12,
        ..synth::FINAL
    };
    let built = synth::build_with_key_sheets(&spec, dir.path());
    let h =
        harness(AppBackend::project(&built.exam_dir, "principal:a11-instructor").expect("open"));
    let ui = &h.ui;
    tall(ui);

    // held: nothing is scored; Results says why and leads to Setup
    assert!(h.app.borrow().results(&h.exam).is_none());
    let hold = h.app.borrow().hold(&h.exam).expect("held");
    assert!(hold.contains("not confirmed"), "{hold}");
    tab(ui, "Results").invoke_accessible_default_action();
    assert!(!ui.global::<ResultsVM>().get_available());
    assert_eq!(
        ui.global::<ResultsVM>().get_empty_title(),
        "Results wait for the key"
    );
    one(ui, "Confirm the key in Setup").invoke_accessible_default_action();
    assert_eq!(ui.global::<App>().get_page(), Page::Setup);

    // Scan: the two key sheets are listed as key sheets, never as students
    h.shell.navigate(Page::Scan);
    let keys: Vec<String> = with_prefix(ui, "Key sheet ")
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert_eq!(keys.len(), 2, "{keys:?}");
    for v in ["A", "B"] {
        assert!(
            keys.iter()
                .any(|l| l.contains(&format!("version {v}, not a student"))
                    && l.contains("waits for your confirmation")),
            "{keys:?}"
        );
    }
    assert!(with_prefix(ui, "Sheet ").len() <= spec.sheets);
    let recon: Vec<(String, String)> = ui
        .global::<ScanVM>()
        .get_recon()
        .iter()
        .map(|kv| (kv.label.to_string(), kv.value.to_string()))
        .collect();
    assert!(
        recon.contains(&("Key sheets, not students".into(), "2".into())),
        "{recon:?}"
    );
    assert!(
        recon.contains(&("Booklets".into(), spec.sheets.to_string())),
        "{recon:?}"
    );
    assert_eq!(ui.global::<ScanVM>().get_key_sheet_count(), 2);
    let activity: Vec<String> = ui
        .global::<ScanVM>()
        .get_activity()
        .iter()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        activity.iter().filter(|a| a.starts_with("KEY ")).count(),
        2,
        "{activity:?}"
    );

    // Setup opens on the key summary of the held exam; version A's three rows wait
    h.shell.navigate(Page::Setup);
    let su = ui.global::<SetupVM>();
    assert!(su.get_ks_open() && su.get_ks_pending());
    assert!(
        su.get_ks_source().contains("not confirmed"),
        "{}",
        su.get_ks_source()
    );
    let rows = key_rows(ui);
    assert_eq!(rows.len(), usize::from(synth::QUESTIONS), "{rows:?}");
    let waiting: Vec<&String> = rows
        .iter()
        .filter(|l| l.contains("needs your decision"))
        .collect();
    assert_eq!(waiting.len(), 3, "{waiting:?}");
    let row = |q: u16| {
        rows.iter()
            .find(|l| l.starts_with(&format!("Question {} version A:", q + 1)))
            .cloned()
            .unwrap_or_default()
    };
    assert!(
        row(synth::KEY_SHEET_MULTI).contains("several marks"),
        "{}",
        row(synth::KEY_SHEET_MULTI)
    );
    assert!(
        row(synth::KEY_SHEET_BLANK).contains("no mark"),
        "{}",
        row(synth::KEY_SHEET_BLANK)
    );
    assert!(
        row(synth::KEY_SHEET_FAINT).contains("needs your decision"),
        "{}",
        row(synth::KEY_SHEET_FAINT)
    );
    // every row carries its scan, straightened, with the marks outlined
    let sum = h.app.borrow().key_sheets(&h.exam).expect("summary");
    assert!(
        sum.versions[0]
            .rows
            .iter()
            .all(|r| r.crop.as_ref().is_some_and(|c| c.width > 2 * c.height))
    );
    // confirming waits for the three decisions
    let confirm = one(ui, "Confirm version A");
    assert_eq!(confirm.accessible_enabled(), Some(false));
    confirm.invoke_accessible_default_action();
    assert!(
        h.app.borrow().results(&h.exam).is_none(),
        "nothing confirmed yet"
    );
    // the instructor decides: the intended answer for the double mark and the faint mark,
    // void for the empty row
    let k = |q: u16| letter(synth::key(&spec, 0, q));
    let q = |x: u16| format!("Q{:02}", x + 1);
    one(
        ui,
        &format!(
            "{} version A answer {}",
            q(synth::KEY_SHEET_MULTI),
            k(synth::KEY_SHEET_MULTI)
        ),
    )
    .invoke_accessible_default_action();
    one(ui, &format!("{} version A void", q(synth::KEY_SHEET_BLANK)))
        .invoke_accessible_default_action();
    one(
        ui,
        &format!(
            "{} version A answer {}",
            q(synth::KEY_SHEET_FAINT),
            k(synth::KEY_SHEET_FAINT)
        ),
    )
    .invoke_accessible_default_action();
    assert!(
        key_rows(ui)
            .iter()
            .all(|l| !l.contains("needs your decision"))
    );
    let confirm = one(ui, "Confirm version A");
    assert_eq!(confirm.accessible_enabled(), Some(true));
    confirm.invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Version A confirmed") && msg.contains("B waits"),
        "{msg}"
    );
    assert!(
        h.app.borrow().results(&h.exam).is_none(),
        "still held until B"
    );

    // version B: every row read; confirming it publishes the key and scores the exam
    one(ui, "Key sheets version B, ready to confirm").invoke_accessible_default_action();
    assert_eq!(su.get_ks_version_letter(), "B");
    one(ui, "Confirm version B").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("Key confirmed") && msg.contains("published"),
        "{msg}"
    );
    let r = h.app.borrow().results(&h.exam).expect("scored");
    assert_eq!(r.stats.students, spec.sheets as u32);
    assert!(r.key_label.contains("in force"), "{}", r.key_label);
    assert!(h.app.borrow().hold(&h.exam).is_none());
    // the confirmation is recorded with the operator and the key-sheet scans
    let (confirmations, voided) = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            let ks = &p.session.key_sheets;
            let c: Vec<(String, usize)> = ks
                .confirmed
                .iter()
                .map(|c| (c.by.clone(), c.scans.len()))
                .collect();
            let k = p.session.key.as_ref().expect("key");
            (
                c,
                k.forms[0][usize::from(synth::KEY_SHEET_BLANK)] == omr_app::model::KeyCell::Void,
            )
        })
        .expect("project");
    assert_eq!(
        confirmations,
        vec![
            ("principal:a11-instructor".to_string(), 1),
            ("principal:a11-instructor".to_string(), 1)
        ]
    );
    assert!(voided, "the empty row was voided as decided");
    // the scores equal grading the same students with the same key as a file
    let mut def = synth::definition(&spec);
    def.key = omr_app::KeySource::Provided {
        forms: (0..2)
            .map(|f| omr_app::FormKey {
                form: ["A", "B"][f].into(),
                answers: (0..synth::QUESTIONS)
                    .map(|q| {
                        if f == 0 && q == synth::KEY_SHEET_BLANK {
                            "*".to_string()
                        } else {
                            letter(synth::key(&spec, f, q))
                        }
                    })
                    .collect(),
            })
            .collect(),
    };
    def.exam_id = def.derived_exam_id();
    let filed = dir.path().join("with-key-file/project.omrproj");
    grade(
        &def,
        &dir.path().join(format!("scans-{}", spec.folder)),
        &filed,
    );
    let other = ExamProject::open_read_only(&filed).expect("reopen");
    let pct = |p: &ExamProject| -> Vec<String> {
        let sc = p.scoring.as_ref().expect("scoring");
        let run = sc.run.as_ref().expect("run");
        run.students
            .iter()
            .map(|s| format!("{:?}", s.final_pct))
            .collect()
    };
    let ours = h.app.borrow().with_project(&h.exam, pct).expect("project");
    assert_eq!(
        ours,
        pct(&other),
        "the same scores as with the key as a file"
    );
}

/// §10.1 in Setup: "Read key from answer sheet…" reads the instructor's key-sheet files for an
/// exam graded with an inferred key, shows the key they give (nothing changes yet) and, once
/// confirmed, makes them the exam's own key sheets (C7: their originals stored with the scans,
/// the key version published with the key sheets as its source and the instructor's name), no
/// longer INFERRED. A student's sheet named as a key sheet gives no key: its grid shows no
/// key-sheet marker (never a wrong key).
#[test]
fn req_a11_real_backend_reads_a_key_from_answer_sheets_in_setup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 8,
        ..synth::QUIZ
    };
    let scans = synth::write_scans(&spec, &dir.path().join("scans"));
    let project = dir.path().join("quiz/project.omrproj");
    grade(&synth::definition(&spec), &scans, &project);
    let keys = synth::write_key_sheets(&spec, &dir.path().join("keys"), false);
    let h = harness(AppBackend::project(&project, "principal:a11-instructor").expect("open"));
    let ui = &h.ui;
    tall(ui);
    let su = ui.global::<SetupVM>();
    let inferred = |h: &Harness| {
        h.app
            .borrow()
            .results(&h.exam)
            .expect("results")
            .key_label
            .to_uppercase()
            .contains("INFERRED")
    };
    assert!(inferred(&h));
    h.shell.navigate(Page::Setup);
    assert!(!su.get_has_keysheets());

    // a student's sheet named as a key sheet: refused, the key is untouched
    one(ui, "Read key from answer sheet…").invoke_accessible_default_action();
    assert!(su.get_ks_bar_open());
    one(ui, "Key-sheet files").set_accessible_value(scans.join("scan-000.jpg").to_str().unwrap());
    one(ui, "Read key sheets").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(msg.contains("No key could be read"), "{msg}");
    assert!(su.get_ks_open());
    assert_eq!(su.get_ks_scans().row_count(), 1);
    assert_ne!(su.get_ks_scans().row_data(0).unwrap().refusal, "");
    assert_eq!(
        one(ui, "Confirm").accessible_enabled(),
        Some(false),
        "nothing to confirm"
    );
    one(ui, "Put aside").invoke_accessible_default_action();
    assert!(ui.global::<App>().get_message().contains("unchanged"));
    assert!(inferred(&h));

    // the instructor's two key sheets
    one(ui, "Read key from answer sheet…").invoke_accessible_default_action();
    let both = format!(
        "{};{}",
        keys[0].to_str().unwrap(),
        keys[1].to_str().unwrap()
    );
    one(ui, "Key-sheet files").set_accessible_value(&both);
    one(ui, "Read key sheets").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("version A and B") && msg.contains("nothing changes until you confirm"),
        "{msg}"
    );
    assert!(su.get_ks_open() && su.get_ks_preview());
    assert!(
        su.get_ks_source().contains("not confirmed"),
        "{}",
        su.get_ks_source()
    );
    assert!(inferred(&h), "nothing changes before the confirmation");
    let rows = key_rows(ui);
    assert_eq!(rows.len(), usize::from(synth::QUESTIONS));
    assert!(rows.iter().all(|l| l.contains(", read, key ")), "{rows:?}");
    one(ui, "Confirm every version").invoke_accessible_default_action();
    let msg = ui.global::<App>().get_message().to_string();
    assert!(
        msg.contains("in force") && msg.contains("confirmed by you"),
        "{msg}"
    );
    assert!(!inferred(&h), "the key is the instructor's now");
    assert!(
        su.get_has_keysheets(),
        "the key sheets are the exam's own now, shown confirmed"
    );
    // C7: the audited path, as with key sheets graded with the exam: the key version names its
    // source and every key-sheet scan, whose original the project stores.
    let provenance = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            let ev = p
                .store()
                .expect("store")
                .events()
                .expect("events")
                .into_iter()
                .filter(|e| {
                    e.event_type == "KeyVersionPublished" && e.payload["source"] == "key_sheet"
                })
                .collect::<Vec<_>>();
            let scans = ev
                .last()
                .and_then(|e| e.payload["scans"].as_array().cloned())
                .unwrap_or_default();
            (ev.len(), scans.len(), p.session.key_sheets.sheets.len())
        })
        .expect("project");
    assert_eq!(
        provenance,
        (1, 2, 2),
        "one key version from both key sheets"
    );
    let key: Vec<Vec<String>> = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            let k = p.session.key.as_ref().expect("key");
            k.forms
                .iter()
                .map(|f| f.iter().map(omr_app::model::KeyCell::text).collect())
                .collect()
        })
        .expect("project");
    for (f, answers) in key.iter().enumerate().take(2) {
        for (q, a) in answers.iter().enumerate() {
            assert_eq!(
                *a,
                letter(synth::key(&spec, f, q as u16)),
                "form {f} Q{}",
                q + 1
            );
        }
    }
}

/// The Scan screen's tile images that are page thumbnails (wider than the 1 × 1 placeholder).
fn shown_thumbnails(ui: &AppWindow) -> usize {
    let images = ui.global::<ScanVM>().get_thumb_images();
    images.iter().filter(|i| i.size().width > 1).count()
}

#[test]
fn req_a11_real_scan_thumbnails_are_decoded_off_the_ui_thread() {
    // The first look at Scan decoded every page thumbnail on the UI thread (about 850 ms on a
    // 60-sheet exam), and so did the end of a grading run with Scan on show. With the binary's
    // spawner a worker decodes them: the screen shows at once with placeholders, and the
    // thumbnails arrive through the pump.
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 6,
        folder: "thumbs",
        ..MIDTERM
    };
    let built = synth::build(&spec, dir.path());
    let (spawn, held) = held_spawner();
    let mut app = AppBackend::project(&built.project, "principal:a11").expect("open");
    app.set_spawner(spawn);
    let h = harness(app);
    let ui = &h.ui;
    tab(ui, "Scan").invoke_accessible_default_action();
    let tiles = ui.global::<ScanVM>().get_thumbs().row_count();
    assert_eq!(tiles, 6);
    assert_eq!(
        shown_thumbnails(ui),
        0,
        "placeholders while the worker decodes"
    );
    assert_eq!(held.borrow().len(), 1, "one decode job for the screen");
    // shown again before the worker is done: nothing is asked twice
    h.shell.navigate(Page::Scan);
    assert_eq!(held.borrow().len(), 1);
    assert!(!h.shell.pump(), "nothing has arrived");
    // the worker runs on its own thread; the pump fills the tiles in
    let work = held.borrow_mut().pop().expect("the decode job");
    std::thread::spawn(work).join().expect("worker");
    assert!(h.shell.pump());
    assert_eq!(shown_thumbnails(ui), tiles);
    // the thumbnails the worker made are those of the pages, as the UI thread made them
    let batch = h.app.borrow().batch_progress(&h.exam).expect("batch");
    let pages: Vec<Vec<u8>> = h
        .app
        .borrow()
        .with_project(&h.exam, |p| {
            (0..p.session.pages.len())
                .map(|i| {
                    let img = p.page_image(i).expect("page image");
                    omr_app::crops::thumbnail(&img, 120).rgba.to_vec()
                })
                .collect()
        })
        .expect("project");
    for t in &batch.thumbnails {
        let rgba = &t.image.as_ref().expect("a thumbnail").rgba;
        assert!(pages.iter().any(|p| p[..] == rgba[..]), "{}", t.serial);
    }
    // cached: shown again, nothing more to decode
    h.shell.navigate(Page::Scan);
    assert!(held.borrow().is_empty());
    assert_eq!(shown_thumbnails(ui), tiles);
}

/// A click on a Scan tile opens the item of that sheet, not the first item of its code in the
/// queue (another student's, opened under the clicked sheet's name).
#[test]
fn req_a11_real_backend_a_tile_opens_its_own_sheet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let spec = synth::Spec {
        sheets: 24,
        folder: "tiles",
        ..MIDTERM
    };
    let built = synth::build(&spec, dir.path());
    let h = harness(AppBackend::project(&built.project, "principal:a11").expect("open"));
    let ui = &h.ui;
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1100.0));
    tab(ui, "Scan").invoke_accessible_default_action();
    // every tile with a code whose sheet is not the first of that code in the queue
    let tiles: Vec<(String, String, String)> = ui
        .global::<ScanVM>()
        .get_thumbs()
        .iter()
        .filter(|t| t.state == 1)
        .map(|t| {
            (
                t.label.to_string(),
                t.serial.to_string(),
                t.code.to_string(),
            )
        })
        .collect();
    assert!(tiles.len() >= 3, "{tiles:?}");
    let mut other_sheet = 0;
    for (label, serial, code) in tiles {
        let first = Lane::ALL.iter().find_map(|&l| {
            let q = h.app.borrow().review_queue(&h.exam, l);
            q.groups
                .iter()
                .find(|g| g.code == code)
                .and_then(|g| g.items.first())
                .map(|i| i.id)
        });
        let first_serial = first.map(|id| h.item(id).serial).unwrap_or_default();
        if first_serial != serial {
            other_sheet += 1;
        }
        h.shell.navigate(Page::Scan);
        click(ui, &one(ui, &label));
        assert_eq!(ui.global::<App>().get_page(), Page::Review, "{label}");
        let id = h.shell.current_item().expect("an item");
        let it = h.item(id);
        assert_eq!(it.serial, serial, "{label} opened booklet {}", it.serial);
        assert_eq!(it.code, code, "{label}");
    }
    assert!(other_sheet > 0, "a sheet that is not the first of its code");
}
