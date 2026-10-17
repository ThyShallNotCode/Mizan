//! `shell-slint`: the Slint desktop shell (spec §5.2, ADR-001). Binding and blitting only: the
//! view-models and every decision come from a [`backend::Backend`]: `omr-app` through
//! [`app_backend::AppBackend`], or the synthetic [`demo::DemoBackend`].
//!
//! R6 audited exception: Slint's generated code expands `allow(unsafe_code)` internally, which a
//! crate-level `forbid` rejects (E0453). The crate therefore uses `deny(unsafe_code)` and the only
//! `allow` is scoped to the generated-code module below; handwritten code stays unsafe-free.
#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod generated {
    slint::include_modules!();
}

/// The Slint-generated window, globals, structs and enums (`ui/*.slint`).
pub mod ui {
    pub use crate::generated::*;
}

pub mod app_backend;
pub mod backend;
pub mod bind;
pub mod crops;
pub mod demo;
pub mod keymap;
pub mod screenshot;
