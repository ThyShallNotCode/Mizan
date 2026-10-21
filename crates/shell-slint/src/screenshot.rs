//! Headless screenshots: every screen rendered with Slint's software renderer at 1440 × 900 into
//! PNG files, with no display (IMPLEMENTATION.md A11, ADR-001 S7). Time is frozen, so
//! `Theme.reduce-motion` is set: animated colours would otherwise be captured mid-transition
//! (DESIGN §6). Any [`Backend`] can be captured: the demo ([`SHOTS`]) or the real core
//! ([`real_shots`]).

use crate::backend::{Backend, Decision, ExamId, Lane};
use crate::bind::{Shell, home_path};
use crate::demo::{CS211, DemoBackend, MATH101};
use crate::host::{Host, Prefs};
use crate::ui::{self as s, AppWindow};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const WIDTH: u32 = 1440;
pub const HEIGHT: u32 = 900;

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::ZERO
    }
}

/// A screen to capture: file stem, page, exam and an optional review set-up.
#[derive(Debug, Clone, Copy)]
pub struct Shot {
    pub name: &'static str,
    pub page: s::Page,
    /// The exam to show; `None` = the backend's default exam.
    pub exam: Option<&'static str>,
    /// For review shots: the lane and the n-th open item of that lane.
    pub review: Option<(Lane, usize)>,
    /// For the setup shot: the mapping cell to select (row, CLO); `None` = the first cell
    /// with a note.
    pub cell: Option<(usize, usize)>,
    /// For review shots: choose the first option other than the suggestion, so the frame shows
    /// the override-reason prompt (§13.4).
    pub override_choice: bool,
    /// Open the screen's bar: Add scans on Scan, Exports on Results, the New-exam form on
    /// Exams.
    pub bar: bool,
    /// Show the first exam whose key comes from key sheets (§10.1): on Setup its key summary,
    /// on Scan its batch with the key sheets listed as such.
    pub keysheet: bool,
    /// The Remark migration (§3.5, §23.7) with the workbooks given to [`render_shots_with`]:
    /// 0 none, 1 the New-exam form's import, 2 Setup's import, 3 the parallel run in Results.
    pub remark: u8,
    /// The New-exam card with the files of an exam to grade ([`ShotFiles::card`]), its scans
    /// checked: 0 the card as it opens, 1 the scans only (the answer key asked), 2 the scans
    /// with the key, roster and CLO files (nothing asked, ready to grade).
    pub card: u8,
    /// The completion notice of the run that ended (a `--grade` run), over the screen.
    pub notice: bool,
}

/// Every screen, plus the identity and blind-verify review frames.
pub const SHOTS: &[Shot] = &[
    Shot {
        name: "exams",
        page: s::Page::Exams,
        exam: Some(CS211),
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "setup",
        page: s::Page::Setup,
        exam: Some(CS211),
        review: None,
        // the cell with an Arabic note: Q18 × CLO4
        cell: Some((17, 3)),
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "scan",
        page: s::Page::Scan,
        exam: Some(MATH101),
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "review",
        page: s::Page::Review,
        exam: Some(CS211),
        review: Some((Lane::Marks, 0)),
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "review-identity",
        page: s::Page::Review,
        exam: Some(CS211),
        review: Some((Lane::Identity, 0)),
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "review-blind",
        page: s::Page::Review,
        exam: Some(CS211),
        review: Some((Lane::Marks, 19)),
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "results",
        page: s::Page::Results,
        exam: Some(CS211),
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "outcomes",
        page: s::Page::Outcomes,
        exam: Some(CS211),
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    Shot {
        name: "settings",
        page: s::Page::Settings,
        exam: Some(CS211),
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
    // the New-exam card with an invented proposal: a question open, Grade exam waiting
    Shot {
        name: "exams-new",
        page: s::Page::Exams,
        exam: Some(CS211),
        review: None,
        cell: None,
        override_choice: false,
        bar: true,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: false,
    },
];

/// The screens of the real core: every screen, plus one review frame per lane that has open
/// items (an empty lane would only show the empty state). There is no blind-verify frame:
/// omr-app does not draw blind samples yet. An exams folder with no exam has one shot,
/// exams-empty: its other screens are empty states with nothing to show.
pub fn real_shots(backend: &dyn Backend) -> Vec<Shot> {
    if backend.dashboard().exams.is_empty() {
        return vec![Shot {
            name: "exams-empty",
            page: s::Page::Exams,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar: false,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        }];
    }
    let mut shots = vec![
        Shot {
            name: "exams",
            page: s::Page::Exams,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar: false,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        },
        Shot {
            name: "setup",
            page: s::Page::Setup,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar: false,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        },
        Shot {
            name: "scan",
            page: s::Page::Scan,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar: false,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        },
    ];
    let exam = backend.default_exam();
    let open = |lane: Lane| {
        exam.as_ref().is_some_and(|e| {
            backend
                .review_queue(e, lane)
                .groups
                .iter()
                .any(|g| !g.items.is_empty())
        })
    };
    for (lane, name) in [
        (Lane::Marks, "review"),
        (Lane::Identity, "review-identity"),
        (Lane::Capture, "review-capture"),
        (Lane::Scoring, "review-scoring"),
    ] {
        if open(lane) {
            shots.push(Shot {
                name,
                page: s::Page::Review,
                exam: None,
                review: Some((lane, 0)),
                cell: None,
                override_choice: false,
                bar: false,
                keysheet: false,
                remark: 0,
                card: 0,
                notice: false,
            });
        }
    }
    if open(Lane::Marks) {
        shots.push(Shot {
            name: "review-override",
            page: s::Page::Review,
            exam: None,
            review: Some((Lane::Marks, 0)),
            cell: None,
            override_choice: true,
            bar: false,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        });
    }
    for (name, page, bar) in [
        ("results", s::Page::Results, false),
        ("results-export", s::Page::Results, true),
        ("outcomes", s::Page::Outcomes, false),
        ("settings", s::Page::Settings, false),
        ("scan-add", s::Page::Scan, true),
        ("exams-new", s::Page::Exams, true),
    ] {
        shots.push(Shot {
            name,
            page,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar,
            keysheet: false,
            remark: 0,
            card: 0,
            notice: false,
        });
    }
    // an exam graded with its key sheets: the key summary to confirm, and its batch
    if key_sheet_exam(backend).is_some() {
        for (name, page) in [
            ("setup-keysheet", s::Page::Setup),
            ("scan-keysheet", s::Page::Scan),
        ] {
            shots.push(Shot {
                name,
                page,
                exam: None,
                review: None,
                cell: None,
                override_choice: false,
                bar: false,
                keysheet: true,
                remark: 0,
                card: 0,
                notice: false,
            });
        }
    }
    shots
}

/// The Remark migration's screens (§3.5, §23.7), taken with the workbooks given to
/// [`render_shots_with`]: the New-exam form's import, Setup's import and the parallel run.
pub fn remark_shots() -> Vec<Shot> {
    [
        ("exams-remark", s::Page::Exams, 1),
        ("setup-remark", s::Page::Setup, 2),
        ("results-remark", s::Page::Results, 3),
    ]
    .into_iter()
    .map(|(name, page, remark)| Shot {
        name,
        page,
        exam: None,
        review: None,
        cell: None,
        override_choice: false,
        bar: remark == 1,
        keysheet: false,
        remark,
        card: 0,
        notice: false,
    })
    .collect()
}

/// The New-exam card made from the files of an exam to grade ([`ShotFiles::card`]): with its
/// scans only, the answer key is the question that waits (exams-new-ask); with its key, roster
/// and CLO files (and the student-number scheme typed, when given) nothing is asked and Grade
/// exam is ready (exams-new-ready).
pub fn card_shots() -> Vec<Shot> {
    [("exams-new-ask", 1), ("exams-new-ready", 2)]
        .into_iter()
        .map(|(name, card)| Shot {
            name,
            page: s::Page::Exams,
            exam: None,
            review: None,
            cell: None,
            override_choice: false,
            bar: true,
            keysheet: false,
            remark: 0,
            card,
            notice: false,
        })
        .collect()
}

/// The completion notice of a grading run that ended (`--grade`), over the Scan screen that
/// followed it.
pub fn notice_shot() -> Shot {
    Shot {
        name: "scan-notice",
        page: s::Page::Scan,
        exam: None,
        review: None,
        cell: None,
        override_choice: false,
        bar: false,
        keysheet: false,
        remark: 0,
        card: 0,
        notice: true,
    }
}

/// The first exam of the dashboard whose key comes from key sheets.
pub fn key_sheet_exam(backend: &dyn Backend) -> Option<ExamId> {
    backend
        .dashboard()
        .exams
        .into_iter()
        .map(|e| e.id)
        .find(|id| backend.key_sheets(id).is_some())
}

/// The smallest window every screen must still fit (DESIGN §9: "works down to 1280×800").
pub const MIN_WIDTH: u32 = 1280;
pub const MIN_HEIGHT: u32 = 800;

/// Installs the headless platform at 1440 × 900. Call once per process, before any window
/// exists.
pub fn install_platform() -> Result<Rc<MinimalSoftwareWindow>, String> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        window: window.clone(),
    }))
    .map_err(|e| format!("cannot install the headless platform: {e}"))?;
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    Ok(window)
}

/// Renders the window's current state into RGB8 pixels at its current size.
pub fn capture(window: &MinimalSoftwareWindow) -> Vec<Rgb8Pixel> {
    let size = window.size();
    let (w, h) = (size.width.max(1), size.height.max(1));
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; (w * h) as usize];
    // Two passes: list views instantiate their rows during the first layout.
    for _ in 0..2 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, w as usize);
        });
    }
    buffer
}

/// Encodes RGB8 pixels as a PNG file.
pub fn write_png(path: &Path, pixels: &[Rgb8Pixel], width: u32, height: u32) -> Result<(), String> {
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        rgba.extend_from_slice(&[p.r, p.g, p.b, 255]);
    }
    let size = tiny_skia::IntSize::from_wh(width, height).ok_or("bad size")?;
    let pixmap = tiny_skia::Pixmap::from_vec(rgba, size).ok_or("bad pixmap")?;
    let png = pixmap.encode_png().map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))
}

/// Puts the shell into the state of `shot`.
pub fn stage<B: Backend + ?Sized>(shell: &Shell, backend: &RefCell<B>, shot: &Shot) {
    stage_with(shell, backend, shot, &ShotFiles::default());
}

/// The files some shots read: Remark's graded workbooks ([`Shot::remark`]) and the files of an
/// exam to grade ([`Shot::card`]).
#[derive(Debug, Clone, Default)]
pub struct ShotFiles {
    /// The New-exam form's import and the parallel run (of the default exam).
    pub workbooks: Vec<PathBuf>,
    /// Setup's import: shown for the first exam of the dashboard it applies to (else the
    /// default exam, with why it does not apply); the New-exam workbooks when empty.
    pub setup: Vec<PathBuf>,
    /// The New-exam card's files; without them a card shot shows the card as it opens.
    pub card: Option<CardFiles>,
}

/// The files an instructor brings to a new exam, for the New-exam card's shots.
#[derive(Debug, Clone)]
pub struct CardFiles {
    /// The scans: a folder or files.
    pub scans: Vec<PathBuf>,
    pub key: PathBuf,
    pub roster: PathBuf,
    pub clo: PathBuf,
    /// The student-number scheme typed under *Advanced options* on the ready card (the
    /// synthetic numbers start with 9, which the IAU scheme refuses); empty: none typed.
    pub id_scheme: String,
}

/// [`stage`], with the files the Remark and New-exam card shots read.
pub fn stage_with<B: Backend + ?Sized>(
    shell: &Shell,
    backend: &RefCell<B>,
    shot: &Shot,
    given: &ShotFiles,
) {
    shell.close_remark();
    // the notice first: taking it reloads the exam it ran on, before the shot is staged
    shell.show_notice(shot.notice);
    let files = if shot.remark == 2 && !given.setup.is_empty() {
        &given.setup
    } else {
        &given.workbooks
    };
    let books = files.iter().map(|p| home_path(p)).collect::<Vec<_>>();
    let books = books.join(";");
    // Setup's import: the first exam it applies to
    let exam = (shot.remark == 2 && !files.is_empty())
        .then(|| applicable_exam(backend, files))
        .flatten();
    stage_screen(shell, backend, shot, exam);
    if shot.bar && shot.page == s::Page::Exams && !backend.borrow().is_demo() {
        fill_card(shell, shot.card, given.card.as_ref());
    }
    if shot.remark > 0 && !files.is_empty() {
        match shot.remark {
            1 | 2 => shell.remark_import(&books),
            _ => shell.remark_compare(&books),
        }
        shell.navigate(shot.page);
    }
}

/// Types the files of an exam to grade into the New-exam card as `card` asks ([`Shot::card`]);
/// card 0, or no files, leaves the card empty as it opens.
fn fill_card(shell: &Shell, card: u8, files: Option<&CardFiles>) {
    use crate::backend::CardField as F;
    let Some(f) = files.filter(|_| card > 0) else {
        shell.fill_card(&[]);
        return;
    };
    let scans = f.scans.iter().map(|p| home_path(p)).collect::<Vec<_>>();
    let scans = scans.join(";");
    let (key, roster, clo) = (home_path(&f.key), home_path(&f.roster), home_path(&f.clo));
    let mut fields = vec![(F::Scans, scans.as_str())];
    if card > 1 {
        fields.extend([
            (F::Key, key.as_str()),
            (F::Roster, roster.as_str()),
            (F::Clo, clo.as_str()),
        ]);
        if !f.id_scheme.is_empty() {
            fields.push((F::IdScheme, f.id_scheme.as_str()));
        }
    }
    shell.fill_card(&fields);
}

/// The first exam of the dashboard whose key the import of `files` may replace.
fn applicable_exam<B: Backend + ?Sized>(backend: &RefCell<B>, files: &[PathBuf]) -> Option<ExamId> {
    let req = crate::backend::RemarkRequest {
        workbooks: files.to_vec(),
        points: crate::backend::RemarkPoints::FromWorkbooks,
        handwritten: Vec::new(),
    };
    let exams: Vec<ExamId> = backend
        .borrow()
        .dashboard()
        .exams
        .into_iter()
        .map(|e| e.id)
        .collect();
    let mut found = None;
    for id in exams {
        let mut b = backend.borrow_mut();
        let fits = b
            .read_remark_import(Some(&id), &req)
            .is_ok_and(|v| v.can_apply);
        b.discard_remark_import(Some(&id));
        if fits {
            found = Some(id);
            break;
        }
    }
    found
}

fn stage_screen<B: Backend + ?Sized>(
    shell: &Shell,
    backend: &RefCell<B>,
    shot: &Shot,
    exam: Option<ExamId>,
) {
    let wanted = if let Some(e) = exam {
        Some(e)
    } else if shot.keysheet {
        let b = backend.borrow();
        b.dashboard()
            .exams
            .into_iter()
            .map(|e| e.id)
            .find(|id| b.key_sheets(id).is_some())
    } else {
        None
    };
    let bars = || {
        shell.set_bars(
            shot.bar && shot.page == s::Page::Scan,
            shot.bar && shot.page == s::Page::Results,
            shot.bar && shot.page == s::Page::Exams,
        )
    };
    let Some(exam) = wanted
        .or_else(|| shot.exam.map(|e| ExamId(e.into())))
        .or_else(|| backend.borrow().default_exam())
    else {
        // an exams folder with no exam: the New-exam card still opens
        bars();
        shell.navigate(shot.page);
        return;
    };
    shell.select_exam(&exam);
    bars();
    if let Some((lane, n)) = shot.review {
        let q = backend.borrow().review_queue(&exam, lane);
        let id = q
            .groups
            .iter()
            .flat_map(|g| g.items.iter())
            .nth(n)
            .map(|i| i.id);
        shell.set_lane(lane);
        shell.select_item(id);
        if shot.override_choice
            && let Some(item) = id.and_then(|i| backend.borrow().review_item(&exam, i))
        {
            let suggested = item.suggestion.as_ref().map(|s| s.decision);
            if let Some(o) = (0..item.bubbles.len() as u8)
                .map(Decision::Option)
                .find(|d| Some(*d) != suggested)
            {
                shell.choose(o);
            }
        }
    }
    shell.show_key_sheets(shot.keysheet);
    if shot.page == s::Page::Setup {
        let cell = shot.cell.or_else(|| {
            let setup = backend.borrow().exam_setup(&exam)?;
            setup.mapping.iter().enumerate().find_map(|(r, row)| {
                row.cells
                    .iter()
                    .position(|c| !c.note.is_empty())
                    .map(|c| (r, c))
            })
        });
        let (row, col) = cell.unwrap_or((0, 0));
        shell.select_cell(row, col);
    }
    shell.navigate(shot.page);
}

/// The window as the desktop app has it, with file dialogs and the file manager, so the shots
/// show the Browse buttons and Open folder: neither is ever opened here, and no preference is
/// read or saved.
pub fn display_host() -> Host {
    Host {
        pick: Some(Rc::new(|_, _| {})),
        reveal: Some(Rc::new(|_| Ok(()))),
        save_prefs: None,
        prefs: Prefs::default(),
    }
}

/// Renders every screen of [`SHOTS`] in the given themes (`false` = light) into
/// `<dir>/<screen>-<theme>.png` at 1440 × 900, returning the paths written.
pub fn render_all(dir: &Path, themes: &[bool]) -> Result<Vec<PathBuf>, String> {
    render_all_at(dir, themes, (WIDTH, HEIGHT))
}

/// As [`render_all`], at another window size; files other than 1440 × 900 are named
/// `<screen>-<theme>-<w>x<h>.png`.
pub fn render_all_at(
    dir: &Path,
    themes: &[bool],
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    let backend: Rc<RefCell<dyn Backend>> = Rc::new(RefCell::new(DemoBackend::new()));
    render_shots(dir, themes, size, backend, SHOTS, None)
}

/// Renders `shots` of any backend (the demo's [`SHOTS`], or [`real_shots`]); `only` keeps the
/// shots with these names. The headless platform is installed here, once per process.
pub fn render_shots(
    dir: &Path,
    themes: &[bool],
    size: (u32, u32),
    backend: Rc<RefCell<dyn Backend>>,
    shots: &[Shot],
    only: Option<&[String]>,
) -> Result<Vec<PathBuf>, String> {
    render_shots_with(
        dir,
        themes,
        size,
        backend,
        shots,
        only,
        &ShotFiles::default(),
    )
}

/// [`render_shots`], with the files the [`remark_shots`] and [`card_shots`] read.
pub fn render_shots_with(
    dir: &Path,
    themes: &[bool],
    (width, height): (u32, u32),
    backend: Rc<RefCell<dyn Backend>>,
    shots: &[Shot],
    only: Option<&[String]>,
    files: &ShotFiles,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let window = install_platform()?;
    window.set_size(PhysicalSize::new(width, height));
    let suffix = if (width, height) == (WIDTH, HEIGHT) {
        String::new()
    } else {
        format!("-{width}x{height}")
    };
    let ui = AppWindow::new().map_err(|e| e.to_string())?;
    ui.global::<s::Theme>().set_reduce_motion(true);
    let shell = Shell::bind(&ui, backend.clone());
    shell.set_host(display_host());
    ui.show().map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    let wanted = |name: &str| only.is_none_or(|o| o.iter().any(|n| n == name));
    for &dark in themes {
        ui.global::<s::Theme>().set_dark(dark);
        for shot in shots.iter().filter(|s| wanted(s.name)) {
            stage_with(&shell, &*backend, shot, files);
            // background analyses (flag stability) report before the capture: a screenshot
            // never catches "computing…"
            if backend.borrow_mut().settle() {
                shell.refresh_all();
            }
            let pixels = capture(&window);
            let path = dir.join(format!(
                "{}-{}{suffix}.png",
                shot.name,
                if dark { "dark" } else { "light" }
            ));
            write_png(&path, &pixels, width, height)?;
            written.push(path);
        }
    }
    ui.hide().map_err(|e| e.to_string())?;
    Ok(written)
}
