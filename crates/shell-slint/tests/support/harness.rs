//! The real-core harness of the desktop tests: a headless window bound to an [`AppBackend`],
//! and the background work it starts held for the test to run (the test plays the binary's
//! part, R4).

#![allow(dead_code)]

use super::synthetic_exam as synth;
use omr_app::ExamProject;
use shell_slint::app_backend::{AppBackend, Spawn};
use shell_slint::backend::{Backend, ExamId};
use shell_slint::bind::Shell;
use shell_slint::ui::{AppWindow, Theme};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::Path;
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
