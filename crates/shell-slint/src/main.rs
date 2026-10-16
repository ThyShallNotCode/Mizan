//! `omr-desktop`: the Slint shell (spec §5.2). Binding and blitting only; logic lives in `omr-app`.
//!
//! R6 audited exception: Slint's generated code expands `allow(unsafe_code)` internally, which a
//! crate-level `forbid` rejects (E0453). The crate therefore uses `deny(unsafe_code)` and the only
//! `allow` is scoped to the generated-code module below; handwritten code stays unsafe-free.
#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod ui {
    slint::include_modules!();
}

use slint::ComponentHandle;
use ui::AppWindow;

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.run()
}
