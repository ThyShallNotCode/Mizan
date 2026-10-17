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
use shell_slint::ui::{App, AppWindow, ExamsVM, Page, ResultsVM, ReviewVM, ScanVM, Theme};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::a11::{key, one, selected_queue_row, tab, with_prefix};
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
    // the review screen says why its queue is empty (never "no open items")
    h.shell.navigate(Page::Review);
    assert!(
        ui.global::<ReviewVM>()
            .get_empty_text()
            .contains("being graded")
    );
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
    run_held(&h, &held);
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
