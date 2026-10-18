//! `omr-outcomes`: the learning-outcome (CLO/PLO/SO) engine (spec §11).
//!
//! A pure function: (per-assessment L3 scores with the §10.1 step-1 artefacts, mapping set,
//! attainment policy, framework versions) → [`AttainmentResult`]. CLO attainment never reads
//! L4–L6 (curves, overrides, misconduct penalties), and every comparison against τ and T is
//! exact (`Ratio<i128>`); only confidence intervals use `f64`.
//!
//! - [`framework`]: NQF-KSA domains (K/S/V), ABET EAC SO 1–7 and CAC SO 1–6, PLOs, CLO sets,
//!   CLO → PLO maps (I/P/M) and PLO → SO crosswalks (§11.1);
//! - [`evidence`]: assessment evidence built from `omr_scoring::ScoreRun`s, roster, sections;
//! - [`mapping`]: `ItemCloMap`/`ItemOutcomeMap` with weights, notes, evidence, rationale and
//!   approval status, plus the §11.2 lints (§11.2, §11.8);
//! - [`policy`]: attainment policy objects, templates, cascade and versioning (§11.4);
//! - [`attainment`]: P_{n,c}, class methods, Wilson/t/bootstrap CIs, groups (§11.3);
//! - [`rollup`]: PLO, credit-hour, exit-exam and framework roll-ups, crosswalk (§11.3, §11.5);
//! - [`report`]: TP-154 B.1 and A.1 tables (§11.7).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod attainment;
pub mod evidence;
pub mod framework;
pub mod mapping;
pub mod policy;
pub mod report;
pub mod rollup;

pub use attainment::{
    AttainmentResult, ClassStats, CloAttainment, Eamu, FcarFlag, GroupKey, GroupResult, StudentClo,
    class_stats, compute_attainment, eamu, mean_of_section_rates, pool_proportions,
    student_performance, suppress_count,
};
pub use evidence::{
    AssessmentEvidence, EvidenceItem, EvidenceStatus, IndirectEvidence, ItemEvidence, Offering,
    RosterEntry, SectionInfo, StudentEvidence, Track,
};
pub use framework::{
    Clo, CloPloMap, CloSet, Course, Crosswalk, Domain, FrameworkOutcome, OutcomeFramework, Plo,
};
pub use mapping::{
    ItemCloMap, ItemOutcomeMap, MappingMode, MappingSet, MappingSource, MappingStatus,
};
pub use policy::{AttainmentPolicy, ClassMethod, PolicyPatch, StudentMethod};
pub use report::{A1Table, B1Row, B1Table, tp154_a1, tp154_b1};
pub use rollup::{
    CourseContribution, CrosswalkRow, credit_hour_rollup, crosswalk_rows, exit_exam_attainment,
    exit_exam_blend, framework_rollup, mapped_grade_share, plo_course_level, plo_student_level,
};
