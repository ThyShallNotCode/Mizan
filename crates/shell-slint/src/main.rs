//! `omr-desktop`: the Slint shell (spec §5.2). Binding and blitting only; logic lives in `omr-app`.
//!
//! ```text
//! omr-desktop [--theme light|dark]                     windowed (winit), demo data
//! omr-desktop --screenshot <dir> [--theme light|dark] [--size WxH]
//!                                                      every screen to <dir>/<screen>-<theme>.png
//! ```
//!
//! The generated UI code lives in the library (`shell_slint::ui`, the audited R6 exception);
//! this binary has no generated code, so it forbids unsafe code outright.
#![forbid(unsafe_code)]

use shell_slint::backend::Backend;
use shell_slint::bind::Shell;
use shell_slint::demo::DemoBackend;
use shell_slint::screenshot;
use shell_slint::ui::{AppWindow, Theme};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

const USAGE: &str = "usage: omr-desktop [--theme light|dark] [--screenshot <dir> [--size WxH]]\n\
  --screenshot <dir>  render every screen headlessly (software renderer, 1440x900) to PNGs\n\
  --size <WxH>        screenshot window size, e.g. 1280x800 (the smallest supported)\n\
  --theme <t>         light or dark (screenshots: both when omitted)";

struct Args {
    screenshot: Option<PathBuf>,
    theme: Option<bool>,
    size: Option<(u32, u32)>,
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w >= 320 && h >= 240 && w <= 7680 && h <= 4320).then_some((w, h))
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        screenshot: None,
        theme: None,
        size: None,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => {
                out.screenshot = Some(args.next().ok_or("--screenshot needs a directory")?.into())
            }
            "--theme" => {
                out.theme = Some(match args.next().as_deref() {
                    Some("light") => false,
                    Some("dark") => true,
                    other => return Err(format!("--theme must be light or dark, not {other:?}")),
                })
            }
            "--size" => {
                let v = args.next().unwrap_or_default();
                out.size = Some(
                    parse_size(&v).ok_or(format!("--size must look like 1280x800, not {v:?}"))?,
                );
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("omr-desktop: {e}");
            }
            eprintln!("{USAGE}");
            return if e.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };
    if let Some(dir) = args.screenshot {
        let themes: Vec<bool> = match args.theme {
            Some(t) => vec![t],
            None => vec![false, true],
        };
        let size = args.size.unwrap_or((screenshot::WIDTH, screenshot::HEIGHT));
        return match screenshot::render_all_at(&dir, &themes, size) {
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
        };
    }
    match run_windowed(args.theme.unwrap_or(false)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omr-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_windowed(dark: bool) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.global::<Theme>().set_dark(dark);
    let backend: Rc<RefCell<dyn Backend>> = Rc::new(RefCell::new(DemoBackend::new()));
    let shell = Shell::bind(&ui, backend);
    shell.start_live_updates();
    ui.run()
}
