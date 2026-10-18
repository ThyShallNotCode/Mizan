//! `omr-sim`: Semi-synthetic generator (§23.3): renders filled sheets with realistic marks (fills,
//! light fills, erasures, ticks, crosses, dots, stray marks, pen) and simulates capture (rotation,
//! translation, scale, shear, perspective, blur, noise, illumination, gamma, JPEG, streaks) with
//! ground truth known by construction. Test/bench only (rule R8: never shipped in release
//! artefacts).
//!
//! ```no_run
//! use omr_forms::{ExamKey, Template, TemplateSpec};
//! use omr_model::{CancelMode, ExamId};
//! use omr_sim::{CorpusConfig, write_corpus};
//!
//! let t = Template::build(TemplateSpec::standard(ExamId::new_v7(), 60, 5, CancelMode::EraseOnly))?;
//! let key = ExamKey::from_bytes([7; 32]);
//! let summary = write_corpus(&t, &key, &CorpusConfig::standard(100, 42), "corpus".as_ref())?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod capture;
pub mod corpus;
pub mod paint;
pub mod profile;
pub mod script;
pub mod sheet;
pub mod truth;

pub use corpus::{
    CorpusConfig, CorpusManifest, CorpusSummary, ImageFormat, read_truth, write_corpus,
};
pub use profile::{CaptureParams, Profile};
pub use script::{Difficulty, Expected, Instrument, MarkRates, SheetScript};
pub use sheet::{PaintedPage, SimPage, Simulator};
pub use truth::{PageTruth, QuestionTruth};

use thiserror::Error;

/// Simulation errors.
#[derive(Debug, Error)]
pub enum SimError {
    /// Rendering failed.
    #[error(transparent)]
    Render(#[from] omr_forms::render::RenderError),
    /// The sheet instance is invalid.
    #[error(transparent)]
    Instance(#[from] omr_forms::instance::InstanceError),
    /// The template has no such page.
    #[error("page {0} does not exist")]
    NoSuchPage(u16),
    /// A roster ID does not match the template's ID grid.
    #[error("roster ID {0:?} does not match the template's ID-digit count")]
    Roster(String),
    /// Encoding or serialisation failed.
    #[error("encoding failed: {0}")]
    Encode(String),
    /// File I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A worker panicked or a lock was poisoned.
    #[error("worker failed")]
    Poisoned,
}
