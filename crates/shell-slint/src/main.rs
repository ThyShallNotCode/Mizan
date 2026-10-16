//! `omr-desktop`: the Slint shell (spec §5.2). Binding and blitting only; logic lives in `omr-app`.
#![forbid(unsafe_code)]

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.run()
}
