//! The real-core harness of the desktop tests: a headless window bound to an [`AppBackend`],
//! the background work it starts held for the test to run, and a fake of what the binary
//! supplies from the operating system (the test plays the binary's part, R4).

#![allow(dead_code)]

use super::synthetic_exam as synth;
use omr_app::ExamProject;
use shell_slint::app_backend::{AppBackend, Spawn};
use shell_slint::backend::{Backend, ExamId};
use shell_slint::bind::Shell;
use shell_slint::host::{Host, Pick, PickRequest, Prefs};
use shell_slint::ui::{AppWindow, Theme};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub struct Harness {
    pub ui: AppWindow,
    pub app: Rc<RefCell<AppBackend>>,
    pub shell: Rc<Shell>,
    pub exam: ExamId,
}

/// A harness on the backend's default exam (it must have one).
pub fn harness(app: AppBackend) -> Harness {
    let exam = app.default_exam().expect("an exam");
    harness_for(app, exam)
}

/// A harness for a workspace whatever it holds: `exam` is its default exam, or a name that
/// matches none.
pub fn harness_in(app: AppBackend) -> Harness {
    let exam = app.default_exam().unwrap_or_else(|| ExamId("none".into()));
    harness_for(app, exam)
}

/// A harness for any workspace, even one without an exam (`exam` is then a name that matches
/// none).
pub fn harness_for(app: AppBackend, exam: ExamId) -> Harness {
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

/// A spawner that holds the work until the test runs it, so the state while the project is
/// handed over can be checked first (the test plays the binary's part, R4).
pub type Held = Rc<RefCell<Vec<Box<dyn FnOnce() + Send>>>>;

pub fn held_spawner() -> (Spawn, Held) {
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
pub fn run_held(h: &Harness, held: &Held) {
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

/// Lets the typing on the New-exam card pause long enough for the card to be proposed again
/// (a survey of changed scans starts, held).
pub fn pause_typing() {
    i_slint_backend_testing::mock_elapsed_time(
        shell_slint::bind::CARD_DEBOUNCE + Duration::from_millis(50),
    );
}

/// Runs the survey the New-exam card started last (held) on a thread, and pumps the shell
/// until the card shows its result.
pub fn run_survey(h: &Harness, held: &Held) {
    let work = held.borrow_mut().pop().expect("a survey was started");
    let worker = std::thread::spawn(work);
    let t0 = Instant::now();
    while h.app.borrow().surveying() {
        assert!(
            t0.elapsed() < Duration::from_secs(300),
            "the survey never finished"
        );
        h.shell.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.join().expect("survey worker");
    h.shell.pump();
}

/// [`pause_typing`], then the survey it started, if any: the card as the instructor sees it
/// once the scans are checked.
pub fn settle_card(h: &Harness, held: &Held) {
    pause_typing();
    if h.app.borrow().surveying() {
        run_survey(h, held);
    }
}

/// Grades the synthetic exam's scans into `project` (as `omr grade` does), and closes it.
pub fn grade(def: &omr_app::ExamDefinition, scans: &Path, project: &Path) {
    let opts = omr_app::GradeOptions {
        threads: 2,
        project_dir: Some(project.to_path_buf()),
        snapshot_utc: Some(synth::SNAPSHOT.into()),
        operator: "principal:a11".into(),
        max_pages: None,
        cancel: Default::default(),
    };
    ExamProject::grade(def, &[scans.to_path_buf()], &opts, &|_| {})
        .expect("grading")
        .close()
        .expect("close");
}

/// A file dialog's answer: the paths picked, or `None` for nothing picked.
type Answer = Option<Vec<PathBuf>>;

/// How a dialog gives its answer to the shell.
type Done = Box<dyn FnOnce(Answer)>;

/// What the desktop binary supplies from the operating system, faked: every file dialog asked
/// for is recorded and answered from a queue (nothing picked once it is empty), and every folder
/// shown in the file manager and every save of the preferences is recorded.
#[derive(Default)]
pub struct FakeHost {
    /// The dialogs asked for, in order.
    pub requests: RefCell<Vec<PickRequest>>,
    answers: RefCell<VecDeque<Answer>>,
    /// Dialogs stay open (unanswered) until [`FakeHost::close`] or [`FakeHost::fail_held`].
    pub hold: Cell<bool>,
    held: RefCell<Option<Done>>,
    /// The folders shown in the file manager.
    pub revealed: RefCell<Vec<PathBuf>>,
    /// The preferences as each save gave them.
    pub saved: RefCell<Vec<Prefs>>,
}

impl FakeHost {
    pub fn new() -> Rc<Self> {
        Rc::default()
    }

    /// The host to hand the shell ([`Shell::set_host`]), with `prefs` as read at start.
    pub fn host(self: &Rc<Self>, prefs: Prefs) -> Host {
        let me = self.clone();
        let pick: Pick = Rc::new(move |req, done| {
            me.requests.borrow_mut().push(req);
            if me.hold.get() {
                *me.held.borrow_mut() = Some(done);
            } else {
                let answer = me.answers.borrow_mut().pop_front().flatten();
                done(answer);
            }
        });
        let me = self.clone();
        let reveal = Rc::new(move |dir: &Path| {
            me.revealed.borrow_mut().push(dir.to_path_buf());
            Ok(())
        });
        let me = self.clone();
        let save = Rc::new(move |p: &Prefs| me.saved.borrow_mut().push(p.clone()));
        Host {
            pick: Some(pick),
            reveal: Some(reveal),
            save_prefs: Some(save),
            prefs,
        }
    }

    /// The next dialog picks `paths`.
    pub fn will_pick(&self, paths: &[&Path]) {
        let paths = paths.iter().map(|p| p.to_path_buf()).collect();
        self.answers.borrow_mut().push_back(Some(paths));
    }

    /// The next dialog is closed without a choice.
    pub fn will_cancel(&self) {
        self.answers.borrow_mut().push_back(None);
    }

    /// Closes the dialog held open, with the next answer of the queue.
    pub fn close(&self) {
        let done = self.held.borrow_mut().take().expect("a dialog is open");
        let answer = self.answers.borrow_mut().pop_front().flatten();
        done(answer);
    }

    /// The dialog held open could not show at all: it never answers.
    pub fn fail_held(&self) {
        let done = self.held.borrow_mut().take().expect("a dialog is open");
        drop(done);
    }

    /// The last dialog asked for.
    pub fn last_request(&self) -> PickRequest {
        let last = self.requests.borrow().last().cloned();
        last.expect("a dialog was asked for")
    }
}
