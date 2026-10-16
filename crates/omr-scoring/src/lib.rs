//! `omr-scoring`: the scoring engine (spec §10).
//!
//! The only crate that reads keys (key-blindness rule R2, §4.3). Every function is pure: a
//! key, policy or weight change never re-runs computer vision, and the same inputs always
//! give the same [`ScoreRun`] and Run ID.
//!
//! Layers (§10.1):
//! - **L1** [`ResponseSnapshot`] of reviewed [`L1Response`]s per printed position;
//! - **L2** projection through the [`FormMap`] (π_f, σ_f) into a compact master-space
//!   [`ResponseMatrix`], bound to a [`KeyVersion`] and [`PolicyVersion`];
//! - **L3** exact item scores ([`ItemScoreMatrix`]) by integer table lookup at a common
//!   denominator D;
//! - **L4** [`Adjustments`] in the normative order (voids, excused, choice groups, x%, floors,
//!   curve, clamp, no-decrease, override, hold-harmless);
//! - **L5** [`course`] aggregation with the SIS coursework/final split;
//! - **L6** [`GradeScale`] (built-in Saudi scale) and [`StatusCode`]s.
//!
//! Points are exact rationals ([`Points`] = `Ratio<i64>`, [`Exact`] = `Ratio<i128>`); `f64`
//! appears only in advisory statistics ([`stats`]).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod course;
pub mod curve;
pub mod error;
pub mod form;
pub mod grade;
pub mod item;
pub mod key;
pub mod l3;
pub mod l4;
pub mod num;
pub mod policy;
pub mod response;
pub mod run;
pub mod stats;

pub use course::{
    Component, ComponentMark, CoursePolicy, CourseResult, CourseStudent, ExcusedRule, MissingWork,
    Requirement, RosterStatus, SisPart, aggregate_course, aggregate_student, boundary_list,
};
pub use curve::{CurveSpec, FrozenCurve};
pub use error::ScoringError;
pub use form::{Form, FormMap, FormPosition};
pub use grade::{A1Column, GradeBand, GradeOutcome, GradeScale, StatusCode};
pub use item::{ItemKind, MAX_OPTIONS, MasterItem, SelectMode, mask_of, options_of};
pub use key::{KeyEntry, KeyVersion};
pub use l3::{CompiledKey, ItemScoreMatrix, ItemState};
pub use l4::{
    Adjustments, AssessmentStatus, Counted, Curve, IndividualOverride, SectionScore,
    StudentResult, Void, VoidMode,
};
pub use num::{
    Exact, LetterBasis, Points, RoundingPolicy, RoundingStage, RoundingStrategy, decimal_string,
    exact, parse_points, pts, widen,
};
pub use policy::{
    ChoiceGroup, ChoiceMode, Floors, MultiOnSba, MultiRule, NegativeMarking, PolicyVersion,
    Section,
};
pub use response::{
    Attendance, BookletL1, Cell, L1Response, ResponseMatrix, ResponseSnapshot, RowMeta,
};
pub use run::{
    ItemMeta, Prepared, RowShiftConfig, ScoreRun, prepare, score, score_prepared,
};
