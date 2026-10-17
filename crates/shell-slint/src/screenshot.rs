//! Headless screenshots: every screen rendered with Slint's software renderer at 1440 × 900 into
//! PNG files, with no display (IMPLEMENTATION.md A11, ADR-001 S7). Time is frozen, so
//! `Theme.reduce-motion` is set: animated colours would otherwise be captured mid-transition
//! (DESIGN §6).

use crate::backend::{Backend, ExamId, Lane};
use crate::bind::Shell;
use crate::demo::{CS211, DemoBackend, MATH101};
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
    pub exam: &'static str,
    /// For review shots: the lane and the n-th open item of that lane.
    pub review: Option<(Lane, usize)>,
}

/// Every screen, plus the identity and blind-verify review frames.
pub const SHOTS: &[Shot] = &[
    Shot {
        name: "exams",
        page: s::Page::Exams,
        exam: CS211,
        review: None,
    },
    Shot {
        name: "setup",
        page: s::Page::Setup,
        exam: CS211,
        review: None,
    },
    Shot {
        name: "scan",
        page: s::Page::Scan,
        exam: MATH101,
        review: None,
    },
    Shot {
        name: "review",
        page: s::Page::Review,
        exam: CS211,
        review: Some((Lane::Marks, 0)),
    },
    Shot {
        name: "review-identity",
        page: s::Page::Review,
        exam: CS211,
        review: Some((Lane::Identity, 0)),
    },
    Shot {
        name: "review-blind",
        page: s::Page::Review,
        exam: CS211,
        review: Some((Lane::Marks, 19)),
    },
    Shot {
        name: "results",
        page: s::Page::Results,
        exam: CS211,
        review: None,
    },
    Shot {
        name: "outcomes",
        page: s::Page::Outcomes,
        exam: CS211,
        review: None,
    },
    Shot {
        name: "settings",
        page: s::Page::Settings,
        exam: CS211,
        review: None,
    },
];

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
pub fn stage(shell: &Shell, backend: &Rc<RefCell<DemoBackend>>, shot: &Shot) {
    let exam = ExamId(shot.exam.into());
    shell.select_exam(&exam);
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
    }
    if shot.page == s::Page::Setup {
        // the cell with an Arabic note: Q18 × CLO4
        shell.select_cell(17, 3);
    }
    shell.navigate(shot.page);
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
    (width, height): (u32, u32),
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
    let backend = Rc::new(RefCell::new(DemoBackend::new()));
    let dynamic: Rc<RefCell<dyn Backend>> = backend.clone();
    let shell = Shell::bind(&ui, dynamic);
    ui.show().map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    for &dark in themes {
        ui.global::<s::Theme>().set_dark(dark);
        for shot in SHOTS {
            stage(&shell, &backend, shot);
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
