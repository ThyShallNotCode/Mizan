//! `omr-desktop`: the Slint shell (spec §5.2). Binding and blitting only; logic lives in `omr-app`.
//!
//! ```text
//! omr-desktop [--theme light|dark]                     windowed (winit), demo data
//! omr-desktop --screenshot <dir> [--theme light|dark]  every screen to <dir>/<screen>-<theme>.png
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

const USAGE: &str = "usage: omr-desktop [--theme light|dark] [--screenshot <dir>]\n\
  --screenshot <dir>  render every screen headlessly (software renderer, 1440x900) to PNGs\n\
  --theme <t>         light or dark (screenshots: both when omitted)";

struct Args {
    screenshot: Option<PathBuf>,
    theme: Option<bool>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        screenshot: None,
        theme: None,
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
        return match screenshot::render_all(&dir, &themes) {
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
