//! `omr-desktop`: the Slint shell (spec §5.2). Binding and blitting only; logic lives in `omr-app`.
//!
//! ```text
//! omr-desktop                                the last workspace (else ~/OMR Exams)
//! omr-desktop --demo                         invented demo data (a banner says so)
//! omr-desktop --project <dir>                one graded exam (its folder or project.omrproj)
//! omr-desktop --workspace <dir>              every graded exam below a folder
//! omr-desktop --grade <scans…> --template iau-ccsit-v1 | --spec spec.json --questions N
//!             [--key key.csv | --infer-key] [--roster r.csv|.xlsx] [--clo clo.csv]
//!             [--out DIR] …                  grades in the background, live on the Scan screen
//! any of these + --screenshot <dir> [--theme light|dark] [--size WxH] [--shots a,b]
//!             [--remark <graded.xlsx… | folder>] [--remark-setup <…>]   the Remark screens
//! ```
//!
//! The generated UI code lives in the library (`shell_slint::ui`, the audited R6 exception);
//! this binary has no generated code, so it forbids unsafe code outright. File reads of the
//! definition inputs, the grading worker thread and everything the window asks of the operating
//! system (native file dialogs, the file manager, the preferences file) live here (rule R4:
//! binaries may); the library only describes them (`shell_slint::host`).
#![forbid(unsafe_code)]
// Windows release builds are GUI programs: no console window opens behind the app. Messages a
// user must see also go to a small window (`tell`); debug builds keep the console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use omr_app::GradeOptions;
use omr_app::define;
use omr_app::exam::ExamDefinition;
use shell_slint::app_backend::{AppBackend, GradeRequest, JobEvent, Spawn, run_grading};
use shell_slint::backend::Backend;
use shell_slint::bind::Shell;
use shell_slint::demo::DemoBackend;
use shell_slint::host::{Host, Pick, PickKind, PickRequest, Prefs};
use shell_slint::screenshot;
use shell_slint::ui::{AppWindow, Page, Theme};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::rc::Rc;

const USAGE: &str = "usage: omr-desktop [--demo | --project <dir> | --workspace <dir> | --grade <scans…> GRADE]
                   [--operator <name>] [--theme light|dark]
                   [--screenshot <dir> [--size WxH] [--shots name,name] [--remark <xlsx…>]]
  (no source)          the workspace used last time, else ~/OMR Exams (created, private)
  --demo               invented demo data, with a banner (nothing is read or written; the
                       default of --screenshot)
  --project <dir>      open one graded exam: its folder or its project.omrproj
  --workspace <dir>    list every graded exam below <dir> (folders holding project.omrproj)
  --grade <scans…>     grade scan files or folders into a new project, live on the Scan screen
  GRADE: --template iau-ccsit-v1 | --spec spec.json, --questions N, [--options N] [--forms A,B]
         [--key key.csv | --infer-key] [--roster roster.csv|.xlsx] [--clo clo.csv]
         [--title T] [--course C] [--term T] [--cancel-mode erase_only|cross_cancels|fill_cancels_check]
         [--multi-review never|always|high_stakes] [--high-stakes] [--id-scheme iau|pnu|ksu|digits:N]
         [--out DIR] (default: a new folder named after the course and title) [--threads N]
  --operator <name>    reviewer recorded in the audit events (default: $USER)
  --screenshot <dir>   render every screen headlessly (software renderer, 1440x900) to PNGs
  --size <WxH>         screenshot window size, e.g. 1280x800 (the smallest supported)
  --shots <names>      only these screens, e.g. review,results
  --remark <xlsx…>     Remark's graded workbooks (files or a folder) for the Remark screens:
                       exams-remark, setup-remark, results-remark (real exams only)
  --remark-setup <xlsx…>  other workbooks for setup-remark (the first exam they apply to)
  --theme <t>          light or dark (screenshots: both when omitted)";

#[derive(Default)]
struct Grade {
    inputs: Vec<PathBuf>,
    template: Option<String>,
    spec: Option<PathBuf>,
    questions: Option<u16>,
    options: Option<u8>,
    forms: Option<Vec<String>>,
    key: Option<PathBuf>,
    infer_key: bool,
    roster: Option<PathBuf>,
    clo: Option<PathBuf>,
    title: Option<String>,
    course: Option<String>,
    term: Option<String>,
    cancel_mode: Option<String>,
    multi_review: Option<String>,
    high_stakes: bool,
    id_scheme: Option<String>,
    out: Option<PathBuf>,
    threads: usize,
}

enum Source {
    /// No source given: the last workspace, else the default one.
    Default,
    Demo,
    Project(PathBuf),
    Workspace(PathBuf),
    Grade(Box<Grade>),
}

struct Args {
    source: Source,
    operator: Option<String>,
    screenshot: Option<PathBuf>,
    theme: Option<bool>,
    size: Option<(u32, u32)>,
    shots: Option<Vec<String>>,
    /// `--remark`: Remark's graded workbooks (files, or a folder of them) for the Remark
    /// screenshots (the import on the New-exam form and in Setup, the parallel run).
    remark: Vec<PathBuf>,
    /// `--remark-setup`: the workbooks for Setup's import shot (the first exam they apply to).
    remark_setup: Vec<PathBuf>,
    /// `--workspace` given together with `--grade`: list it beside the new exam.
    workspace: Option<PathBuf>,
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w >= 320 && h >= 240 && w <= 7680 && h <= 4320).then_some((w, h))
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = args.peekable();
    let mut out = Args {
        source: Source::Default,
        operator: None,
        screenshot: None,
        theme: None,
        size: None,
        shots: None,
        remark: Vec::new(),
        remark_setup: Vec::new(),
        workspace: None,
    };
    let mut g = Grade::default();
    let mut grading = false;
    let mut sources = 0;
    while let Some(a) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .filter(|v| !v.starts_with("--"))
                .ok_or(format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--demo" => {
                out.source = Source::Demo;
                sources += 1;
            }
            "--project" => {
                out.source = Source::Project(value("--project")?.into());
                sources += 1;
            }
            "--workspace" => out.workspace = Some(value("--workspace")?.into()),
            "--grade" => {
                grading = true;
                sources += 1;
                while let Some(p) = args.next_if(|v| !v.starts_with("--")) {
                    g.inputs.push(p.into());
                }
                if g.inputs.is_empty() {
                    return Err("--grade needs scan files or folders".into());
                }
            }
            "--template" => g.template = Some(value("--template")?),
            "--spec" => g.spec = Some(value("--spec")?.into()),
            "--questions" => {
                let v = value("--questions")?;
                g.questions = Some(
                    v.parse()
                        .map_err(|_| format!("--questions must be a number, not {v:?}"))?,
                );
            }
            "--options" => {
                let v = value("--options")?;
                g.options = Some(
                    v.parse()
                        .map_err(|_| format!("--options must be a number, not {v:?}"))?,
                );
            }
            "--forms" => {
                g.forms = Some(
                    value("--forms")?
                        .split(',')
                        .map(|f| f.trim().to_uppercase())
                        .filter(|f| !f.is_empty())
                        .collect(),
                )
            }
            "--key" => g.key = Some(value("--key")?.into()),
            "--infer-key" => g.infer_key = true,
            "--roster" => g.roster = Some(value("--roster")?.into()),
            "--clo" => g.clo = Some(value("--clo")?.into()),
            "--title" => g.title = Some(value("--title")?),
            "--course" => g.course = Some(value("--course")?),
            "--term" => g.term = Some(value("--term")?),
            "--cancel-mode" => g.cancel_mode = Some(value("--cancel-mode")?),
            "--multi-review" => g.multi_review = Some(value("--multi-review")?),
            "--high-stakes" => g.high_stakes = true,
            "--id-scheme" => g.id_scheme = Some(value("--id-scheme")?),
            "--out" => g.out = Some(value("--out")?.into()),
            "--threads" => {
                let v = value("--threads")?;
                g.threads = v
                    .parse()
                    .map_err(|_| format!("--threads must be a number, not {v:?}"))?;
            }
            "--operator" => out.operator = Some(value("--operator")?),
            "--screenshot" => out.screenshot = Some(value("--screenshot")?.into()),
            "--theme" => {
                out.theme = Some(match value("--theme")?.as_str() {
                    "light" => false,
                    "dark" => true,
                    other => return Err(format!("--theme must be light or dark, not {other:?}")),
                })
            }
            "--size" => {
                let v = value("--size")?;
                out.size = Some(
                    parse_size(&v).ok_or(format!("--size must look like 1280x800, not {v:?}"))?,
                );
            }
            "--shots" => {
                out.shots = Some(
                    value("--shots")?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect(),
                )
            }
            "--remark" | "--remark-setup" => {
                let list = if a == "--remark" {
                    &mut out.remark
                } else {
                    &mut out.remark_setup
                };
                while let Some(p) = args.next_if(|v| !v.starts_with("--")) {
                    list.push(p.into());
                }
                if list.is_empty() {
                    return Err(format!(
                        "{a} needs Remark's graded workbooks (.xlsx) or their folder"
                    ));
                }
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if sources > 1 {
        return Err(
            "give one of --demo, --project or --grade (--workspace may join --grade)".into(),
        );
    }
    if grading {
        out.source = Source::Grade(Box::new(g));
    } else if let (Source::Default, Some(w)) = (&out.source, &out.workspace) {
        out.source = Source::Workspace(w.clone());
    } else if matches!(out.source, Source::Default) && out.screenshot.is_some() {
        // screenshots of nothing in particular are of the demo data, never of a real
        // workspace that happens to be the last one used
        out.source = Source::Demo;
    }
    Ok(out)
}

/// The exam definition of `--grade` (the same rules as `omr grade` and the New-exam form).
fn definition(g: &Grade) -> Result<ExamDefinition, String> {
    let questions = g
        .questions
        .ok_or("--grade needs --questions N (the questions this exam uses)")?;
    let sheet = match (&g.template, &g.spec) {
        (Some(t), None) => define::Sheet::Template(t.clone()),
        (None, Some(p)) => define::Sheet::Spec(p.clone()),
        _ => return Err("--grade needs --template iau-ccsit-v1 or --spec spec.json".into()),
    };
    let mut i = define::Inputs::new(sheet, questions);
    i.options = g.options;
    i.forms = g.forms.clone();
    i.key = g.key.clone();
    i.infer_key = g.infer_key;
    i.roster = g.roster.clone();
    i.clo = g.clo.clone();
    i.title = g.title.clone();
    i.course = g.course.clone();
    i.term = g.term.clone();
    i.cancel_mode = g.cancel_mode.clone();
    i.multi_review = g.multi_review.clone();
    i.high_stakes = g.high_stakes;
    i.id_scheme = g.id_scheme.clone();
    let (d, warnings) = define::define(&i).map_err(|e| e.to_string())?;
    for w in warnings {
        eprintln!("omr-desktop: roster: {w}");
    }
    Ok(d)
}

/// `--out`, or a new folder named after the course and title below the workspace (or here).
fn out_dir(g: &Grade, d: &ExamDefinition, workspace: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(o) = &g.out {
        if o.join("project.omrproj").exists() {
            return Err(format!(
                "{} already holds a graded exam; give a new --out folder, or open it with --project",
                o.display()
            ));
        }
        return Ok(o.clone());
    }
    let base = workspace.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok(define::new_exam_dir(&base, d))
}

fn operator(a: &Args) -> String {
    let name = a
        .operator
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok()) // Windows sets USERNAME, not USER
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "operator".into());
    if name.starts_with("principal:") {
        name
    } else {
        format!("principal:{name}")
    }
}

enum Built {
    Demo(Rc<RefCell<DemoBackend>>),
    App(Rc<RefCell<AppBackend>>),
}

impl Built {
    fn dynamic(&self) -> Rc<RefCell<dyn Backend>> {
        match self {
            Built::Demo(d) => d.clone(),
            Built::App(a) => a.clone(),
        }
    }
}

/// Background work of the real backend (adding scans) runs on threads this binary owns (R4).
fn spawner() -> Spawn {
    Box::new(|work| {
        std::thread::Builder::new()
            .name("omr-append".into())
            .spawn(work)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

fn app(mut backend: AppBackend) -> Built {
    backend.set_spawner(spawner());
    Built::App(Rc::new(RefCell::new(backend)))
}

/// The desktop's own preferences file (the last workspace, the folders the file dialogs opened
/// last, …), in the private data folder.
fn settings_file() -> Option<PathBuf> {
    omr_app::keys::key_dir().map(|d| d.join("desktop.json"))
}

/// The preferences saved last time; the defaults when there are none or they are unreadable.
fn load_prefs() -> Prefs {
    settings_file()
        .and_then(|f| std::fs::read(f).ok())
        .map(|b| Prefs::from_json(&b))
        .unwrap_or_default()
}

/// Saves the preferences, readable by the user only (best effort).
fn save_prefs(p: &Prefs) {
    let Some(f) = settings_file() else {
        return;
    };
    if let Some(d) = f.parent() {
        let _ = omr_app::keys::create_private_dir(d);
    }
    let _ = omr_app::keys::write_private(&f, &p.to_json());
}

/// Remembers `w` as the workspace to open next time (best effort).
fn remember_workspace(prefs: &mut Prefs, w: &Path) {
    let Ok(abs) = std::path::absolute(w) else {
        return;
    };
    prefs.remember_workspace(abs);
    save_prefs(prefs);
}

/// The workspace of a launch without a source: the last one if it still exists, else
/// `~/OMR Exams` (created private to the user: it will hold Restricted data).
fn default_workspace(prefs: &Prefs) -> Result<PathBuf, String> {
    if let Some(w) = prefs.workspace.clone().filter(|w| w.is_dir()) {
        return Ok(w);
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .ok_or("no home folder: give the exams folder with --workspace DIR")?;
    let w = PathBuf::from(home).join("OMR Exams");
    omr_app::keys::create_private_dir(&w).map_err(|e| {
        format!(
            "{}: {e}; give the exams folder with --workspace DIR",
            w.display()
        )
    })?;
    Ok(w)
}

fn backend(a: &Args, prefs: &mut Prefs) -> Result<(Built, bool), String> {
    let op = operator(a);
    Ok(match &a.source {
        Source::Demo => (
            Built::Demo(Rc::new(RefCell::new(DemoBackend::new()))),
            false,
        ),
        Source::Project(p) => {
            let b = AppBackend::project(p, &op)?;
            if let Some(w) = shell_slint::app_backend::project_dir(p)
                .parent()
                .and_then(Path::parent)
            {
                remember_workspace(prefs, w);
            }
            (app(b), false)
        }
        Source::Default => {
            let w = default_workspace(prefs)?;
            remember_workspace(prefs, &w);
            (app(AppBackend::workspace(&w, &op)), false)
        }
        Source::Workspace(w) => {
            if !w.is_dir() {
                return Err(format!(
                    "{}: no such folder; give the folder that holds the exam folders",
                    w.display()
                ));
            }
            remember_workspace(prefs, w);
            (app(AppBackend::workspace(w, &op)), false)
        }
        Source::Grade(g) => {
            let def = definition(g)?;
            let out = out_dir(g, &def, a.workspace.as_deref())?;
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("{}: {e}; give a writable --out folder", out.display()))?;
            let req = GradeRequest {
                definition: def.clone(),
                inputs: g.inputs.clone(),
                key_sheets: Vec::new(),
                options: GradeOptions {
                    threads: g.threads,
                    project_dir: Some(out.join("project.omrproj")),
                    snapshot_utc: None,
                    operator: op.clone(),
                    max_pages: None,
                    cancel: Default::default(),
                },
            };
            let (tx, rx) = std::sync::mpsc::channel::<JobEvent>();
            std::thread::Builder::new()
                .name("omr-grade".into())
                .spawn(move || run_grading(req, tx))
                .map_err(|e| format!("cannot start the grading worker: {e}"))?;
            (
                app(AppBackend::grading(&def, rx, a.workspace.as_deref(), &op)),
                true,
            )
        }
    })
}

/// Prints `text` to stderr and, in a Windows release build (no console), also shows it in a
/// window, so a user who started Mizan from the Start menu or Explorer sees why it stopped.
/// When no window of the app can open (often the very reason it stopped), the system's own
/// message box shows it.
fn tell(heading: &str, text: &str) {
    eprintln!("{text}");
    if cfg!(all(windows, not(debug_assertions))) {
        let Ok(w) = shell_slint::ui::ErrorWindow::new() else {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(heading)
                .set_description(text)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        };
        w.set_heading(heading.into());
        w.set_message(text.into());
        let weak = w.as_weak();
        w.on_close(move || {
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
        let _ = w.run();
    }
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            tell("Mizan", USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tell(
                "Mizan cannot start",
                &format!("omr-desktop: {e}\n\n{USAGE}"),
            );
            return ExitCode::from(2);
        }
    };
    let mut prefs = load_prefs();
    let (built, live) = match backend(&args, &mut prefs) {
        Ok(b) => b,
        Err(e) => {
            tell("Mizan cannot start", &format!("omr-desktop: {e}"));
            return ExitCode::FAILURE;
        }
    };
    if let Some(dir) = &args.screenshot {
        return screenshots(&args, dir, &built);
    }
    match run_windowed(args.theme.unwrap_or(false), built.dynamic(), live, prefs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // R10: the windowing system is missing (no display, no X11 or Wayland session)
            tell(
                "Mizan cannot start",
                &format!(
                    "omr-desktop: cannot open a window ({e}). Run omr-desktop from a desktop \
                     session (Windows, macOS, or Linux with X11 or Wayland); on a server, render \
                     the screens to PNG files with --screenshot DIR, or use the `omr` command line"
                ),
            );
            ExitCode::FAILURE
        }
    }
}

fn screenshots(args: &Args, dir: &Path, built: &Built) -> ExitCode {
    let themes: Vec<bool> = match args.theme {
        Some(t) => vec![t],
        None => vec![false, true],
    };
    let size = args.size.unwrap_or((screenshot::WIDTH, screenshot::HEIGHT));
    let shots = match built {
        Built::Demo(_) => screenshot::SHOTS.to_vec(),
        Built::App(a) => {
            // a grading run is captured once it has finished (the Scan screen then shows the
            // whole batch); screenshots stay deterministic
            while a.borrow().grading_in_progress() {
                a.borrow_mut().pump();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Some(e) = a.borrow().grading_error() {
                eprintln!("omr-desktop: grading failed: {e}");
                return ExitCode::FAILURE;
            }
            let mut shots = screenshot::real_shots(&*a.borrow());
            if !args.remark.is_empty() {
                shots.extend(screenshot::remark_shots());
            }
            shots
        }
    };
    let remark = match (workbooks(&args.remark), workbooks(&args.remark_setup)) {
        (Ok(workbooks), Ok(setup)) => screenshot::RemarkFiles { workbooks, setup },
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("omr-desktop: {e}");
            return ExitCode::FAILURE;
        }
    };
    match screenshot::render_shots_with(
        dir,
        &themes,
        size,
        built.dynamic(),
        &shots,
        args.shots.as_deref(),
        &remark,
    ) {
        Ok(paths) => {
            for p in paths {
                println!("{}", p.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("omr-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The workbooks of `--remark`: files as given, a folder's `.xlsx` files in name order (the
/// binary lists the folder, R4).
fn workbooks(given: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for p in given {
        if p.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(p)
                .map_err(|e| {
                    format!(
                        "{}: {e}; give the folder of Remark's workbooks",
                        p.display()
                    )
                })?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|f| {
                    f.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("xlsx"))
                })
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// Opens a native file dialog, parented to the main window, and answers on the UI thread when
/// it closes. The dialog runs on a thread of its own (on Windows and through the desktop
/// portal), so the window keeps painting meanwhile.
fn picker(ui: &AppWindow) -> Pick {
    let weak = ui.as_weak();
    Rc::new(move |req: PickRequest, done| {
        let kind = req.kind;
        let mut d = rfd::AsyncFileDialog::new().set_title(req.title.as_str());
        if let Some(start) = req.start.as_deref().filter(|s| s.is_dir()) {
            d = d.set_directory(start);
        }
        for (name, extensions) in &req.filters {
            d = d.add_filter(name.as_str(), extensions);
        }
        if let Some(ui) = weak.upgrade() {
            d = d.set_parent(&ui.window().window_handle());
        }
        let dialog = async move {
            let picked = match kind {
                PickKind::File => d.pick_file().await.map(|f| vec![f]),
                PickKind::Files => d.pick_files().await,
                PickKind::Folder => d.pick_folder().await.map(|f| vec![f]),
            };
            done(picked.map(|v| v.into_iter().map(|f| f.path().to_path_buf()).collect()));
        };
        // without an event loop there is no dialog: the answer is dropped unanswered, which the
        // shell takes as nothing picked
        let _ = slint::spawn_local(dialog);
    })
}

/// Shows `dir`, an existing folder, in the system's file manager. Never a file: the file
/// manager would open it, or run it.
fn reveal(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{}: no such folder", dir.display()));
    }
    let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let program = if cfg!(windows) {
        // by its full path, never looked up on PATH (its exit code is 1 even when it worked)
        let root = std::env::var_os("SystemRoot")
            .filter(|r| !r.is_empty())
            .ok_or("cannot find Windows Explorer (SystemRoot is not set)")?;
        PathBuf::from(root).join("explorer.exe")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("open")
    } else {
        PathBuf::from("xdg-open")
    };
    let mut child = std::process::Command::new(program)
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot open the file manager ({e})"))?;
    // waited for on a thread of its own, so no finished process is left behind
    std::thread::Builder::new()
        .name("omr-reveal".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// What the window may ask of the operating system: file dialogs, the file manager, and saving
/// the preferences.
fn host(ui: &AppWindow, prefs: Prefs) -> Host {
    Host {
        pick: Some(picker(ui)),
        reveal: Some(Rc::new(reveal)),
        save_prefs: Some(Rc::new(save_prefs)),
        prefs,
    }
}

/// Runs the window until it is closed; then every exam open is saved and closed (each writes
/// its signed checkpoint).
fn run_windowed(
    dark: bool,
    backend: Rc<RefCell<dyn Backend>>,
    live: bool,
    prefs: Prefs,
) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    // the Wayland app id and X11 class, matched by packaging/linux/mizan.desktop (its icon)
    let _ = slint::set_xdg_app_id("mizan");
    ui.global::<Theme>().set_dark(dark);
    let shell = Shell::bind(&ui, backend.clone());
    shell.set_host(host(&ui, prefs));
    if live {
        shell.navigate(Page::Scan);
    }
    shell.start_live_updates();
    let ran = ui.run();
    for e in backend.borrow_mut().close_all() {
        eprintln!("omr-desktop: not saved and closed properly: {e}");
    }
    ran
}
