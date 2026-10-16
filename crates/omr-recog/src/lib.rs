//! `omr-recog`: the recognition engine (§8). This increment implements the **legacy-form path**
//! (§6.10, forms without fiducials) validated on 143 real IAU CCSIT sheets: ring-lattice
//! registration (§8.4 adapted), per-bubble darkness (§8.5–§8.6, bitonal mode §7.2/CAP-003),
//! the per-position baseline β_p (§8.6) and the per-question decision logic (§8.8) with
//! Appendix A exceptions (§13.1). The [`ownform`] module is the own-form path (§6.3–§6.4,
//! §8.2–§8.4): AprilTag corners, Stage A, the verified page code and row ticks, on the same
//! decision core.
//!
//! ```no_run
//! use omr_recog::{LegacyEngine, PageRequest};
//! use omr_imaging::{CaptureKind, decode};
//!
//! let engine = LegacyEngine::iau_ccsit_v1().unwrap();
//! # let bytes: Vec<u8> = Vec::new();
//! let page = decode(&bytes).unwrap();
//! let m = engine.measure_page(&page.image);                 // page stage (pure)
//! let baseline = engine.batch_baseline(&[&m]);              // batch stage (or prior_baseline())
//! let req = PageRequest { n_questions: 29, high_stakes: false, capture: page.capture, page: None };
//! let result = engine.decide(&m, &baseline, &req);
//! # let _ = result;
//! ```
//!
//! Rule R2 (key-blindness): recognition never sees an answer key. Rule R4: no I/O, no threads.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod baseline;
pub mod config;
pub mod decide;
mod exc;
pub mod legacy;
pub mod measure;
pub mod ownform;
pub mod register;

pub use baseline::{Baseline, BaselineSource};
pub use config::{
    BaselineConfig, BaselineFallback, DecisionConfig, LEGACY_CONFIG_VERSION, LegacyConfig,
    MeasureConfig, MultiReview, RegistrationConfig,
};
pub use decide::{GroupDecision, GroupKind, ReviewReason, decide_group, page_level};
pub use legacy::{
    ChoiceRead, GridField, LegacyEngine, LegacyPageResult, PageMeasurement, PageRequest,
};
pub use measure::{BubbleMeasure, Unreadable};
pub use register::{Registration, RegistrationQuality};

use thiserror::Error;

/// Engine construction errors.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecogError {
    /// The template or its reference image is unusable.
    #[error("template: {0}")]
    Template(String),
}
