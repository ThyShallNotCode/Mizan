//! `omr-app`: Application services (§4.3, §4.5): the exam project model, the ingest → recognise
//! → identify → score → outcomes pipeline with its scheduler (rayon), the review queue, and
//! view-models consumed by the shells and the CLI. Owns parallelism and file I/O (rule R4).
//!
//! ```no_run
//! use omr_app::{ExamDefinition, GradeOptions, TemplateChoice, KeySource, ExamProject};
//! use omr_model::CancelMode;
//! # fn demo() -> Result<(), omr_app::AppError> {
//! let mut def = ExamDefinition::new("Final", TemplateChoice::Legacy("iau-ccsit-v1".into()), 29, 5,
//!     vec!["A".into(), "B".into()], CancelMode::EraseOnly);
//! def.key = KeySource::Inferred;
//! let opts = GradeOptions { project_dir: Some("out/project.omrproj".into()), ..GradeOptions::default() };
//! let mut exam = ExamProject::grade(&def, &["scans/".into()], &opts, &|_| {})?;
//! exam.export("out".as_ref())?;
//! # Ok(()) }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod append;
pub mod audit;
pub mod crops;
pub mod define;
pub mod engine;
pub mod exam;
pub mod export;
pub mod infer;
pub mod ingest;
pub mod keys;
pub mod keysheet;
pub mod model;
pub mod outcomes;
pub mod parallel;
pub mod pipeline;
pub mod propose;
pub mod remark;
pub mod review;
pub mod score;
pub mod store;
pub mod survey;
pub mod vm;
pub mod xlsx;

pub use app::{ApplyOutcome, ExamProject};
pub use exam::{
    CloDefinition, CloMapping, DomainLetter, ExamDefinition, FormKey, IdSchemeChoice, KeySheetRule,
    KeySource, MultiReviewPolicy, OutcomeSetup, PolicyChoice, RosterStudent, TemplateChoice,
};
pub use pipeline::{Cancel, GradeOptions, Progress, ProvisionalPage};
pub use review::{Action, Decision, ItemKind, ItemState, Lane, ReviewItem};

use thiserror::Error;

/// Application errors (each maps to a CLI exit code).
#[derive(Debug, Error)]
pub enum AppError {
    /// The exam definition is invalid.
    #[error("exam definition: {0}")]
    Definition(String),
    /// An input file or folder is unusable.
    #[error("input: {0}")]
    Input(String),
    /// The project store refused an operation.
    #[error("project store: {0}")]
    Store(String),
    /// Scoring failed.
    #[error("scoring: {0}")]
    Scoring(String),
    /// A review action was refused.
    #[error("review: {0}")]
    Review(String),
    /// Not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// File I/O.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// Internal error.
    #[error("internal: {0}")]
    Internal(String),
    /// The operator cancelled the run ([`Cancel`]) before anything was stored.
    #[error("cancelled by the operator; nothing was saved")]
    Cancelled,
}

impl AppError {
    /// Process exit code for the CLI: 2 usage/definition, 3 input, 4 store, 5 scoring,
    /// 6 review, 7 not found, 8 I/O, 70 internal, 130 cancelled (as an interrupted command).
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Definition(_) => 2,
            AppError::Input(_) => 3,
            AppError::Store(_) => 4,
            AppError::Scoring(_) => 5,
            AppError::Review(_) => 6,
            AppError::NotFound(_) => 7,
            AppError::Io(_) => 8,
            AppError::Internal(_) => 70,
            AppError::Cancelled => 130,
        }
    }
}
