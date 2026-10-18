//! `omr-forms`: Template model (§6.1, §6.3, §6.5), page code payload and HMAC-SHA384 96-bit tag
//! (§6.4), fiducial layout, pre-print lint (§6.9) and sheet rendering to SVG/PNG/PDF behind the
//! `render` feature (§6.2, §6.8).
//!
//! Rule R2: knows nothing about answer keys. Rule R4: no file or network I/O and no threads; fonts
//! are compiled in.
//!
//! ```
//! use omr_forms::{Template, TemplateSpec};
//! use omr_model::{CancelMode, ExamId};
//!
//! let spec = TemplateSpec::standard(ExamId::new_v7(), 60, 5, CancelMode::EraseOnly);
//! let t = Template::build(spec).unwrap();
//! let q = t.question_bubbles(0);
//! assert_eq!(q.len(), 5);
//! assert_eq!(t.hash_hex().len(), 96);
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod apriltag;
pub mod geometry;
pub mod hash;
pub mod instance;
pub mod layout;
pub mod legacy;
pub mod lint;
pub mod pagecode;
pub mod spec;
pub mod units;

#[cfg(feature = "render")]
pub mod render;

pub use instance::{Personalisation, PersonalisationError, SheetInstance};
pub use layout::{
    AnswerBubble, Block, BlockKind, BuildError, CodeSlot, Corner, FiducialTag, FormBubble,
    IdBubble, LayoutManifest, PageArt, PageGeometry, QuestionGeom, RowTick, Side, Template,
};
pub use lint::{LintFinding, LintLevel, LintRule};
pub use pagecode::{ExamKey, PageCode, PageCodeError, ParsedPageCode};
pub use spec::{
    CodeDiagonal, DigitLabels, Direction, LabelSet, OptionCounts, Paper, SpecError, TemplateSpec,
};
pub use units::{PtUm, RectUm, Um};
