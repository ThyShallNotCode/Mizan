//! `omr-identity`: Identity extraction and roster reconciliation (§9): roster snapshots (§9.2), ID-grid column
//! decoding, Saudi student-ID schemes and national ID/Iqama detection (§9.2, §9.9), fusion and
//! one-sheet-per-student assignment (§9.5, §9.6).
//!
//! Rule R2: never sees an answer key. Rule R4: no file or network I/O, no thread pools; the
//! caller parses CSV/XLSX and passes records.
//!
//! ```
//! use omr_identity::*;
//! use omr_model::{ColumnState, IdColumnRead, IdGridRead};
//!
//! let scheme = StudentIdScheme::iau();
//! let records = vec![RosterRecord { student_key: "2220001234".into(), name: "A".into(), ..Default::default() }];
//! let (roster, _report) = RosterSnapshot::build(records, &scheme, &ImportOptions::default()).unwrap();
//! let grid = IdGridRead { columns: "2220001234".bytes().enumerate().map(|(i, b)| IdColumnRead {
//!     column: i as u8, state: ColumnState::Single, marked: vec![b - b'0'], logits: vec![] }).collect() };
//! let ev = BookletEvidence { grid: Some(grid), ..Default::default() };
//! let d = decode_booklet(&ev, &roster, &scheme, &PriorContext::default(), &IdentityPolicy::default());
//! assert_eq!(d.student.unwrap().0, "2220001234");
//! // One channel only: confirm on a contact sheet (shadow mode); no exception raised.
//! assert_eq!(d.status, IdentityStatus::NeedsConfirmation);
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod assign;
pub mod channels;
pub mod decode;
pub mod national;
pub mod normalize;
pub mod roster;
pub mod scheme;

pub use assign::{
    Assigned, BatchOutcome, BookletAssignment, BookletIdentity, assign_batch, assign_rosterless,
};
pub use channels::WriterErrorKind;
pub use decode::{
    BookletEvidence, Candidate, ChannelUse, Finding, IdentityDecision, IdentityNote,
    IdentityPolicy, IdentityStatus, PriorContext, ReadRelation, ReadSummary, SubReason, TypedId,
    TypedSource, decode_booklet, decode_rosterless, mark_roster_diff, nats, q,
};
pub use national::{
    ColumnVerdict, IdentifierClass, NationalIdKind, classify_identifier, is_national_id,
    mask_digits, national_id_column, national_id_kind,
};
pub use normalize::{digits_only, normalize_id, normalize_name};
pub use roster::{
    ImportIssue, ImportOptions, ImportReport, RosterDiff, RosterError, RosterRecord, RosterRow,
    RosterSnapshot, SisStatus, Track,
};
pub use scheme::{
    CheckDigit, DecodedId, FieldKind, FieldRule, FieldValue, SchemeError, SchemeSpec,
    SchemeViolation, StudentIdScheme,
};
